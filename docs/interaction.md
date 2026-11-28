# Interaction contract

The rules for choosing, watching and returning by TV remote, touch, or mouse and
keyboard, including the remote's key tables and the desktops' (Linux, macOS,
Windows) wheel, shortcuts, windows and displays. Platform follow-ups (#87 TV, #88
tablet, #89 desktop) build on this contract rather than defining their own.
[Accessibility](testing.md#accessibility-journeys) has the assistive-technology journeys.

Each rule is marked **Now** (implemented today) or **Planned** (agreed in #86,
not yet built). Update the mark in the change that delivers it. The key tables
describe the code as it is. [Verification](#verification) lists the tests that
cover the **Now** rules. Unmarked statements, such as the principles, are goals that the
marked rules implement; each section states where current behavior still falls
short.

## Contents

- [Principles](#principles), [input modes](#input-modes) and
  [destinations](#destinations)
- Guide: [choosing and inspecting](#choosing-and-inspecting),
  [secondary actions](#secondary-actions)
- Inputs: [remote keys](#remote-keys), [desktop keyboard](#desktop-keyboard),
  [pointer and wheel](#pointer-and-wheel)
- Player: [full screen](#full-screen), [panel timeouts](#panel-timeouts),
  [getting back](#getting-back)
  ([Samsung exit policy](#samsung-tizen-exit-policy),
  [pending device checks](#pending-device-checks))
- Layout: [layout classes](#layout-classes),
  [presentation changes](#what-a-presentation-change-keeps)
- Sound and windows: [which player is heard](#which-player-is-heard),
  [closing](#closing), [displays](#displays-and-hosts),
  [casting](#cast-live-tv), [interruptions](#interruptions-and-the-background)
- [Text entry](#text-entry),
  [empty states](#empty-and-unavailable-states),
  [admission](#admission-and-capability), [verification](#verification)

## Principles

- **Choose to preview, choose again to watch.** In the TV Guide’s preview
  layout, choosing a live channel plays it in the preview, on every input and
  every device; choosing the playing channel again goes full screen on the
  same stream. In the overlay layout and the channel list, which already draw
  over full-screen video, choosing a channel watches it full screen unless the
  viewer has chosen to keep the guide or list open.
- **Layout is appearance, not behavior.** Window size and orientation change
  how content is arranged, never what an action does. The guide layout decides
  only where a chosen channel first plays.
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

- **Now:** all three modes (`iptv_presentation::input_mode::InputMode`). Each
  press records finger versus mouse, trackpad or pen; on desktop, mouse movement
  anywhere (and a sideways wheel in the guide) counts as Pointer, without moving
  focus. A remote's Back counts as a key. The main window starts in Touch on
  phones and tablets (Android and iOS) and in Keys on TVs, desktops and
  terminals; a separate desktop player window starts in Pointer. A tablet with a
  keyboard and mouse switches between modes as each is used.
- **Now:** the first key after touch or a pointer moves focus onto a row on
  screen if the focused channel has scrolled out of view.
- **Now:** a remote key held while touch opens or closes a PIN prompt does
  nothing more until it is released or pressed again: no tap, hold or step
  back acts on the screen the prompt covered or exposed
  ([Taps and holds](#taps-and-holds)).

## Destinations

The navigation rail holds a stable set of destinations
(`iptv_presentation::browse::rail_items`): Search, Live TV, Movies, Shows,
Continue watching, Recently added, Recordings and Settings. Playlists are managed
under Settings.

- **Now:** Live TV is the start screen. With **Play the last channel on start**
  (`general.resume_last_channel`, on by default) the last channel starts full
  screen over it. The guide behind shows Favorites, so Back lands there on the
  channel, if it is one of the playlist's favorites and that category is offered
  (`channels.show_favorites`); otherwise All channels
  (`iptv_presentation::lists::resume_entry`). The list is chosen once the channel
  plays (after any PIN); later favorite edits and viewer choices are left alone.
- **Now:** Movies and Shows appear before the first playlist and whenever an
  enabled playlist is Xtream Codes or Stalker (even with movie and series import
  off). Once every enabled playlist is M3U, which never carries them, they leave
  the rail and the Movies and Shows screen's own navigation. Recordings follows
  the host's [library rule](platform-support.md#record-now-and-the-recordings-library).
  Continue watching and Recently added appear when any enabled playlist has movies
  or series.
- **Now:** **Settings → Appearance → Categories** has separate switches for
  Favorites, Recently watched, and All channels in the guide, player channel
  list, and multiview picker. The player list follows the **Channel logos**
  setting used in the guide.
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
  replay starts at once and goes full screen. On an iPad, a tap on the guide
  beside the open groups column only closes the column.
- **Now:** Up and Down wrap between the first and last channel rows and between
  the first and last displayed groups, on every device.
- **Now:** in the overlay layout and the channel list, **After choosing a
  channel** (`guide.after_choosing`, on desktops and TVs; **Go full screen** by
  default) can keep the guide or the list open after tuning (**Keep it open**).
  A phone or a tablet always goes full screen. It replaced
  `guide.stay_on_guide` and `guide.stay_on_list` (#281): either one stored on
  reads as **Keep it open** while the new choice is not stored. In a compact
  window the guide always shows its preview layout.
- **Now:** moving the focus never tunes a channel unless **Autoplay**
  (`guide.autoplay`) is on, in the guide or the channel list; it then tunes the
  focused live channel after `guide.switch_delay`, and moving on before then
  cancels it. On a desktop and a TV both settings are under Settings → Advanced.
  A phone or a tablet shows them with the guide's settings only while a keyboard
  or D-pad is connected; without one, autoplay is off whatever is stored, and the
  stored values are kept.
- **Now:** opening a program's details or menu never starts playback
  ([Secondary actions](#secondary-actions)).
- **Now:** a screen reader's activation (TalkBack's double tap, VoiceOver's
  and Narrator's activate) is a choice like a tap: a channel row or a
  program cell plays as the table says, and the guide's buttons act. The app
  hears it as a press with no position (`accessibility_visibility`).
- **Now (Android):** the pinned NSH routes hover by `pointerEvents` as it
  routes touches (#192), so touch exploration reaches the guide past the inert
  full-screen layers above it.

### Touch gestures in the guide

- **Now:** a drag decides its direction after 10 points: mostly vertical scrolls
  the channels, mostly horizontal pans the timeline. A diagonal goes the way of
  its larger part and stays that way (on an iPad the channel list's scrolling
  waits for that decision, so a 1.5:1 diagonal pans too: NSH's iOS pan gate,
  `nsh.rev`). A drag released while moving flings and slows down; one the system
  cancels (notification shade, call) stops where it is, without a fling. A drag
  starting on a channel or a program, past or upcoming, never chooses it; under
  the threshold a touch is still a tap (`iptv_presentation::guide::timeline`).
- **Now:** crossing the threshold, in the guide or channel list, cancels a
  preview `guide.autoplay` was waiting to start until the focus moves to another
  channel. It is read from the first finger's own touches, so a drag at the
  list's end that scrolls nothing counts, as does a scroll view taking over a
  finger that moved; a still finger's cancel does not
  (`iptv_presentation::guide::timeline::Threshold`). A tap's own choice still
  plays after its switch delay. Scrolling never tunes, records or opens a menu or
  details (`BrowsePlayback::dragged`; `tests/app/tablet_gestures.rs`).
- **Now:** a timeline pan starting on a channel's program on air keeps that
  channel (by identity) for browse playback while focus moves along the row
  (`iptv_presentation::browse_playback::PanAnchor`): a pending choice still plays,
  and panning back to now starts no preview again. Any other focus move (keys, a
  tap, another list, a zap) ends that hold.
- **Now:** a day of listings that a pan pages in keeps the channels where the
  finger scrolled them.

### Changing channel by touch (#285)

- **Now:** on full-screen live video, swipe up for Ch+ and down for Ch−, along
  the list on screen, wrapping as Ch± does. Past the 10-point threshold the banner
  of the target channel (number, name, program on air) follows the finger;
  releasing past a quarter of the screen's height, or flinging, changes channel
  with a light haptic, and bringing it back does nothing. A swipe starts only 40
  points or more from the screen's edges (left to system gestures). A second
  finger cancels it; horizontal drags are left alone
  (`iptv_presentation::touch_zap`). A swipe is never also a tap on the video:
  Android reports the video's press when the finger lifts anywhere inside it, and
  the app drops that one press. A keyboard's or screen reader's press stays a tap.
- **Now:** a replay, an on-demand title, multiview, the guide's preview and
  anything open over the video take no swipe.
- **Now:** a swipe zaps as Ch± does (`browse_playback::zap_by`): the banner names
  the new channel at once and the stream tunes after `guide.switch_delay`, so a
  burst opens only the last stream. The banner shows whatever
  `player.info_on_switch` says; while the play bar shows, the bar's own line names
  the channel instead.
- **Now:** on desktop, tablet and phone (not TV), the play bar's transport row has
  previous (Ch−) and next (Ch+) buttons either side of Play/Pause during live
  playback (moving together into More when narrow). Full-screen video has "Next
  channel" and "Previous channel" accessibility actions for TalkBack and
  VoiceOver, which take swipes themselves; on an iPad the video is also
  adjustable, so VoiceOver's swipe up and down changes channel.
- **Now:** **Swipe to change channel** (`playback.swipe_to_zap`, on by default,
  offered with a touch screen or pointer) turns the swipe off; buttons and actions
  stay.

## Secondary actions

Program and channel menus (favorites, recording, details, replay) need a
visible entry point for touch and pointer users.

- **Now:** in the guide, the channel list and recordings: Menu on a remote, and
  long OK on Android; M on a keyboard, and Shift+F10 on desktop. Long press and
  right-click open the same menus:
  - On guide channel names and timeline cells, a channel header or a player-list
    row, and for the focused timeline cell or player-list row, the compact
    channel actions. A guide timeline cell's menu is for the selected program's
    time block, on every device.
  - On the program title in the guide's header, its program menu. Screen readers
    can invoke the title's menu action; on iPad, VoiceOver calls it **More
    options**.
  - On a group, the group options. Menu or M on the single-playlist All channels
    group adds a playlist.

  During full-screen TV playback, either OK press and Menu open the
  [play bar](#play-bar).
- **Now:** **More options** in either compact menu gives access to the full
  program, channel and group actions, including live replay, details and favorite
  management, for the captured channel and time block. Back returns to the
  compact menu with **More options** focused; Menu dismisses either menu.
- **Now:** a context gesture moves focus to what it opens the menu for;
  `guide.autoplay` then holds on that channel, after the menu or its details
  close, until the viewer moves the focus to another channel. A choice still
  plays.
- **Now:** while touch or a pointer drives the app, a visible "More options"
  control (⋯) opens actions on channel-list rows and recordings rows, and group
  options in the player's channel list. There it shows only on the shown group,
  so a narrow column keeps its names: a click shows another group, whose row
  then has it. Keys hide these controls; they have Menu (M) and Shift+F10.
  Channel rows hide their ⋯ while multiview picks a channel (rows have no menu
  then).
- **Now:** the guide has no overflow buttons in its header. The ruler shows the
  date and time and offers **Now** after browsing away; drag, wheel or keyboard
  shortcuts browse program time.
- Choose **Channel actions** from a program menu to use that program's
  time block. The channel menu opens on Favorites, followed by the recording
  action for the selected block. A live block can start or stop a recording;
  an upcoming block has no recording action. The player list uses the
  live block. On Linux and macOS, **Open in new window** plays the channel
  live in a [separate Patina window](#several-players).
- **Reorder channel** and **Reorder channels** open a draft order in the guide
  from channel, group and program menus. OK or Enter picks up or places a
  channel. Up and Down move it one row, and Left and Right move it to either
  end. Touch and mouse users can drag the handles and choose Save or Cancel.
  The playing channel keeps its player surface throughout the edit.
- **Reorder groups** in a group's menu uses the same controls in the guide's
  groups column. It starts on the group whose menu you opened.
- **Show favorites only** filters that group. **Guide time offset** corrects
  the times in a provider group in 15-minute steps. Existing recordings keep
  their accepted capture windows.
- **Use external player** applies to explicit channel choices on Android and
  Linux, including choosing an archived program and its **Play from the
  beginning** action. Both honor the same group preference: the menu action
  chooses a starting position, not a different player. The captured group,
  channel and program occurrence are rechecked after PIN entry and before handoff:
  focus changes never substitute a target, and a removed or changed one is
  rejected. A full-screen menu can restart the playing channel even when the guide
  last browsed another group; that unrelated group cannot authorize a handoff.
  Android offers installed video apps; Linux tries VLC, then mpv; other hosts play
  in Patina. The option appears only while such a player is installed, or while it
  is on so it can be turned off. If the player was removed, a choice plays in
  Patina and offers **Turn off external player**, only once playback is
  authorized: canceling its PIN plays nothing and leaves no offer. During an
  Android handoff a media playback foreground service keeps Patina's relay
  running.

## Remote keys

What each remote key does on each surface, as a tap and as a hold
(`iptv_nav::Keymap::tv`; in `crates/iptv-app/src/`, `shell/input.rs`,
`playback/player_keys.rs` and `playback/play_bar.rs`). A viewer can rebind keys
in Settings → Remote control (`remote_editor.rs`), which these tables do not
cover.

That page, which also holds the replay seek keys (`catchup.seek_keys`), exists
only while there are keys to set up (`settings::keys`): always on a TV (Remote
control) and a desktop (Keyboard, without the remote-only colour keys, Play,
Pause, Stop and Record); on a phone or tablet only while the device reports a
connected hardware keyboard (Keyboard) or D-pad or game controller (Remote
control), whatever input was used last. Single-key shortcuts work on an Android
keyboard, so they stay remappable while one is attached (WCAG 2.1.4). The
long-press time and a hold in the editor are offered only where keys report
release. Changed keys have no row of their own; the editor's row sums them up.

"–" in a table means the key does nothing there. Keys not listed for a surface
fall back to the global row: Search opens search, Red toggles the favorite,
digits enter a channel number, Play/Pause, Play and Pause play and pause,
Stop stops playback, ⏮ (Previous) is the previous channel and ⏭ (Next) goes
back to live.

### Taps and holds

- **Hold** means the key stays down for `remote.long_press` (400 ms by
  default). Only keys that report press and release tell a hold from a tap: an
  Android remote's or keyboard's and a browser TV's, but not a modified shortcut
  (`Shift+Left`), which reports its press only and is bound for a tap. A desktop
  keyboard reports no holds and has Shift and Menu (`M`) stand-ins.
- A key with a hold action on the current surface waits: released earlier, it
  taps on release; held to the threshold, decided by an auto-repeat or by a timer
  (for remotes that send no repeats), its hold action acts once. Later repeats and
  the release do nothing.
- A key without a hold action acts at once and on each repeat, so a held arrow
  keeps moving. Exceptions that act once per press: Menu (remote or `M`), so a
  menu or the play bar does not flicker, and Favorite toggles, including custom
  bindings.
- A waiting press belongs to the surface it went down on: the keymap's context
  and what owns input within it (the info card and control strip share a context
  but are different surfaces). If the surface differs when the press is decided
  (a touch played a channel or opened More controls, a prompt opened, the guide
  went full screen), the press is dropped, tap and hold alike
  (`iptv_nav::HeldPresses`; logged "dropped: the surface changed"). A surface that
  changes and comes back first counts as unchanged. A dropped Back also takes
  Android's own Back on its release with it. A waiting press replaced by another
  key's (Menu while OK is held on the play bar) stays replaced until released or
  pressed afresh; its repeats start no new press where it did not go down.
- A PIN prompt or the remote editor takes the keys it gets. A press pending from
  before is cut short, and a key the prompt took does nothing more while held (the
  OK that submitted a PIN starts no hold on the video): repeats, release and timer
  stay inert, even after the prompt closes, until the key is released or pressed
  afresh. This holds however the prompt opens and closes, including by touch with
  no key reaching it. A Back held from before the prompt does not dismiss it on
  release, whatever else is pressed meanwhile. A fresh Back closes a PIN prompt
  once, through Android's Back; if touch closes the prompt while Back is down, its
  repeats do nothing and its release takes no step back.
- On a desktop keyboard, a key a PIN prompt took (the Enter that closed it) does
  not repeat until pressed afresh; a held arrow or digit in the prompt acts once.
- A key going down keeps the info card and play bar up until the press is decided,
  even if their timeout is shorter than the hold; input meanwhile (a zap's banner)
  restarts their timeout without shortening that.
- In a replay the seek arrows have no hold: held, they repeat the seek.
- Long Back's release takes no step back.

### Guide keys

| Key | Tap | Hold |
|---|---|---|
| Up / Down | Move between channels, wrapping at the first and last rows. The groups column wraps between its first and last entries. | Repeats |
| Left | Earlier programs, then the groups column, then the rail | The groups column |
| Right | Later programs | Repeats |
| OK | [Choose](#choosing-and-inspecting) | Compact channel actions for the focused program |
| Menu | Compact channel actions; on the groups column, the group's options | – |
| Back | [One step back](#back-order) | Back to the video, when something plays |
| Ch+ / Ch− and Page Up / Down | A page of channels | Repeats |
| Record | Records the focused program if it is on now; on another program it says only a program on now can be recorded | – |
| Guide | – (already the guide) | – |
| Info | – | – |

### Mini-player in the guide

Press Up from Search in the navigation rail to focus the playing video.

| Key | Tap | Hold |
|---|---|---|
| Down | Back to Search in the rail | Repeats |
| Left | The groups column (on release: Left has a hold in the guide) | – (the guide's hold, the groups, does nothing here) |
| Right | The channel rows | Repeats |
| OK | The playing stream full screen, as it plays (no new stream), even if the guide focuses another channel | Opens the play bar full screen |
| Menu | The play bar, full screen | – |
| Back | Back to Search in the rail | The playing stream full screen, as it plays |

### Full-screen live video

| Key | Tap | Hold |
|---|---|---|
| OK | Opens the play bar with Play/pause focused. Over a failed stream, retries. | Same as tap |
| Menu | The play bar (again: hide it) | – |
| Up / Down | Channel up / down (`remote.player_up_down` can make them open the channel list) | Repeats |
| Ch+ / Ch− and Page Up / Down | Channel up / down | Repeats |
| Left | The channel list over the video | The channel list beside the video |
| Right | The previous channel (again: back) | The recently watched channels |
| 0 | Channel number entry | The previous channel |
| Info | The info card alone | – |
| Guide | The guide over the video | – |
| Rewind / Fast-forward | – (nothing to seek live) | – |
| Record | Starts or stops recording the playing program | – |
| Back | Hides the info card or play bar; then back to the guide on the playing channel | Stops playback and returns to the guide |

Channel up and down show the next channel's banner at once and tune after
`guide.switch_delay`, so quick presses skip channels.

### Info card and its action cards

A custom remote binding can open the info card with action cards. The cards
capture Left, Right and OK while they show.

| Key | Tap | Hold |
|---|---|---|
| Left / Right | Move between the action cards (Guide, History, Channels, Favorite, Multiview, More controls) | Repeats |
| OK | Presses the focused action card; with none focused, hides the card | The play bar |
| Up / Down | Channel up / down, the card following | Repeats |
| Menu | The play bar | – |
| Back | Hides the card | Stops playback |

### Replay and seekable video on demand

As full-screen live, except:

| Key | Tap | Hold |
|---|---|---|
| Left / Right (`catchup.seek_keys`: Left/Right, the default) | Choose a seek one time-bar step back or on, shown on the card's bar; it starts after a short pause, or at once on OK | Repeats the step, faster (no hold action) |
| Down / Up (`catchup.seek_keys`: Down/Up) | As Left / Right above, instead of zapping | As above |
| OK | Starts a pending seek at once or opens the play bar with Play/pause focused | Same as tap |
| Rewind / Fast-forward | Seek back / on | Repeats |
| ⏭ (Next) | Back to live (a replay) | – |

A seek and a track choice keep the same player. Only a replay's seek opens a
stream (the archive at the new time).

### Play bar

The same play bar appears on tablet and desktop. On TV it has remote focus. It
captures the arrows and OK while it shows, so they never zap or seek behind it.

| Key | Tap | Hold |
|---|---|---|
| Left / Right | Move between the enabled controls (live skips Rewind and Fast-forward); on the timeline, seek | Repeats |
| Down | The timeline (replay or seekable video on demand); elsewhere stays | – |
| Up | Back to Play/pause | – |
| OK | Presses the focused control; on the timeline, starts the seek and returns to Play/pause | Focus back on Play/pause; the repeats press nothing |
| Menu | Hides the bar | – |
| Back | Closes a chooser the bar opened, then the bar | Stops playback |

The remote skips unavailable controls; the advanced strip names the reason where
one applies (for example "Picture in picture · Play a channel first").

### Choosers, menus and dialogs

| Surface | Up / Down | OK | Left / Right | Back or Menu |
|---|---|---|---|---|
| Chooser (picture size, tracks, sleep timer, audio delay, history) | Move | Picks and closes; focus returns to the control that opened it | – | Closes; focus returns to that control |
| Program and channel menu | Move | Performs the item | – | Closes; focus returns to the program or channel ([More options](#secondary-actions): Back returns to the compact menu) |
| Program details | – | Records the program | – | Closes |
| Channel list over the video | Move | Plays the focused channel | Left: the groups; Right: back to the channels | Back: from the groups to the channels, then closes; Menu: the channel's or group's menu (also Hold OK) |
| PIN prompt | Move between digits and actions | On a digit after digits typed on number keys, submits; otherwise presses the focused key, so a PIN of any length can be entered on the keypad and Continue submits it. Number keys type digits | Move between digits and actions | Back closes; the content it guarded is not played |
| Settings and other screens | Move | Activates | Move between columns | Back returns to where Settings opened, on the same channel |

Ch+ / Ch− in the channel list zap without closing it.

## Desktop keyboard

One compact set, the same on every desktop; `Mod` is Ctrl on Linux and
Windows and Command on macOS.

| Keys | Does |
|---|---|
| Arrows | Move. Up and Down wrap through the TV Guide's channel rows and groups. |
| Enter | [Choose](#choosing-and-inspecting) |
| Escape or Backspace | One step back ([Getting back](#getting-back)) |
| / or Mod+F | Search |
| M, Shift+F10 | Menu for the focused program, channel or group |
| Space | Play or pause |
| = and - | Channel up and down (a page in the guide) |
| P | Previous channel |
| N | Back to live |
| [ and ] | Rewind and fast-forward |
| I | Information |
| G | Back to the guide |
| Home | The programs on now |
| Tab, Shift+Tab | The guide's groups column, and back |
| Shift+← and Shift+→, Mod+← and Mod+→ | An hour, a day through the guide |
| 0–9 | Channel number |
| Ctrl+M | Mute or unmute |
| Ctrl+↑ and Ctrl+↓ (Cmd on macOS) | Volume |
| F | Full screen in Patina's window, and back (main window) |
| O | The guide over the video, and back (main window) |
| F11 (Linux, Windows), Ctrl+Cmd+F (macOS) | The operating system's full screen for the window |
| Mod+, (macOS) | Settings |

- **Now:** the set above (`crates/iptv-app/src/kit/input.rs`,
  `shell/input.rs`). macOS also has its menu bar (`shell/desktop.rs`); Linux
  and Windows draw none.
- **Now: a shortcut belongs to the window that has the keyboard focus.**
  Keys pressed in a second player window control that player only, and the
  main window's keys its own. The view keys F and O exist in the main
  window only.
- **Now:** text fields keep their keys ([Text entry](#text-entry)).

## Pointer and wheel

| Where (desktop) | Vertical wheel or two-finger swipe | Horizontal swipe, or Shift + wheel | Right-click | Visible way in |
|---|---|---|---|---|
| Guide | Moves through the channels | Moves through program time | Channel menu on a channel name or program cell | Long press or Menu key |
| Guide's program title in the header | Nothing | Nothing | Program menu | Long press; More options in compact channel actions |
| Guide's groups column | Scrolls the groups | Nothing | Group options | Long press or Menu key |
| Channel list over the video | Scrolls the channels | Nothing | Channel menu | ⋯ at the end of each row |
| Channel list's groups column | Scrolls the groups | Nothing | Group options | ⋯ on the shown group's row |
| Movies and Shows posters | Scrolls the posters | Nothing | Nothing | — |
| Recordings | Scrolls the recordings | Nothing | The recording's actions | ⋯ at the end of each row |

- **Now:** all rows above (`crates/iptv-app/src/guide/grid/wheel.rs`,
  `screens/channels_view.rs`, `playback/panels.rs`,
  `screens/recordings_view/rows.rs`). A wheel, a right-click or a ⋯ never
  plays, tunes or stops anything.
- **Now:** hovering never changes the channel or the focus that playback
  follows. Moving the mouse over full-screen video shows the player's
  controls for a while.

## Full screen

"Full screen" means the video fills Patina's screen or window. On desktop it
never resizes the operating-system window; that is a separate action
(double-click on the video, F11 or Ctrl+Cmd+F, or the window's own control).

- **Now (desktop):** a double click on the video toggles the operating system's
  full screen for that window, and plays nothing again. Escape first leaves the
  operating system's full screen, then the player's panels, then Patina's full
  screen.

## Panel timeouts

- **Now:** the play bar hides after three seconds without use; a chooser it
  opened keeps it up. The info card hides after `player.controls_panel_timeout`;
  a key keeps it up. These, the banner, the control strip and multiview's pointer
  chrome never hide sooner than Android's recommended accessibility timeout. Their
  own timeouts are fixed on handhelds and desktop; TV keeps them under Advanced
  settings.
- **Now:** with a screen reader on (WCAG 2.2.1), the play bar over full-screen
  video, multiview's pointer chrome (on the bar's timer;
  `iptv_presentation::play_bar`) and the remote's panels
  (`iptv_presentation::panels`) stay until the viewer hides them (for the bar: a
  double tap on the video, Back, or a key off a TV). All of them also stay while
  Android's Switch Access or iPadOS Switch Control runs.

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

- **Now:** visible Back and Close follow the input mode on hosts with a remote
  Back key: a TV used by touch or a pointer shows them, and a remote key hides
  them again. Tablets and desktops always show them, keyboard users included.
  The play bar shows a Back button except on TVs and terminals
  (`Ui::back_controls`). List editing keeps its host's model (dragging with Save
  and Cancel, or OK and the arrows) whichever input drives it.
- **Now:** closing a chooser, menu, PIN prompt or settings screen restores focus
  by content identity (the control, program or channel), not by row position, also
  after the list was refreshed or reordered.

### Back order

**Now:** Back, Escape and Backspace dismiss the nearest surface, one step per
press (`go_back` in `crates/iptv-app/src/shell/back.rs`). The main steps:

1. A channel number being typed.
2. A PIN prompt, then a screen opened over the app (wizard, Movies and
   Shows, Recordings, Settings and its sub-screens).
3. A chooser, then a menu or program details.
4. Search, the channel list over the video, the multiview picker.
5. On desktop, restore the main player's window from the operating system's full
   screen.
6. Over video: the info card or the play bar.
7. Full-screen video: back to where it was chosen (the guide on the playing
   channel, Recordings for a recording, a title's details, the multiview grid for
   a tile). The stream keeps playing in the mini-player or behind the guide.
8. In the guide over the video (the overlay layout while a channel plays), back
   to the video.
9. In the guide: the hint card, the programs on air (from a past or upcoming
   program), the groups column, the rail, then [leave](#leaving-the-app).
   `guide.back_returns_to_now` is retired and its stored value ignored (#281).

Long Back is covered under [Remote keys](#remote-keys); player windows have their
own Escape rule ([Closing](#closing)).

### Returning to playback

- **Now:** from the guide, OK on the mini-player, OK on the playing channel, Back
  in the guide over the video and a hold of Back all return the playing stream
  to full screen without opening it again.
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
  (`vod_view::observe`). With `vod.autoplay_next_episode` on, the next episode
  plays where the viewer is: full screen, or in the preview behind that series'
  details, selected with its season (Play or OK takes it full screen). With
  autoplay off, or when the next episode would need a PIN: from full screen the
  series' details open on it, and on those details playback stops and selects it;
  both say "Ready for the next episode". Anywhere else (a list or shelf, the
  guide, another title's details, settings), playback stops as after the viewer's
  Stop, and the series' card and Continue watching hold the next episode. Its PIN
  is asked only when the viewer plays it (#187).

### Leaving the app

- **Now:** where Back ends navigation (the guide's rail with nothing open, and
  nothing playing behind the overlay layout; the first-run welcome; library
  recovery with details closed), the app registers no Back handler, unless a
  channel number or a pending Long Back release is active.
- **Now (Android):** top-level Back goes to the system by default, returning home
  in one press, with the system predictive preview where available (Android 13/14
  needs the developer animation option; 15+ enables it by default for opted-in
  apps). Handhelds keep predictive back.
- **Now (Android TV):** viewers can enable **Press Back twice to leave**
  (**Settings → General**, `general.double_back_to_exit`, off by default). The app
  then keeps the handler on the guide's rail: the first Back shows an exit
  message, and Back again while the message is visible returns to the launcher.
  The message normally lasts two seconds and follows longer Android
  accessibility timeouts.
- **Now (desktop):** Escape at a root does nothing; the window
  [closes](#closing) the operating system's way.
- **Now (Samsung Tizen):** Return at the top level opens **Close Patina?**
  ([exit policy](#samsung-tizen-exit-policy)).
- The former `general.confirm_exit` and `general.confirm_exit_window` settings
  are not offered, and stored values are ignored.

### Samsung Tizen exit policy

Patina follows Samsung's [Terminating Applications
guide](https://developer.samsung.com/smarttv/develop/guides/fundamentals/terminating-applications.html)
for both hosts (decided 2026-11-25). Both retain the handler at their roots and
ask "Close Patina?" with No focused; the NSH host includes recovery with details
closed. Back or No dismisses the question and Yes leaves. The bridge moves to No
with Left, to Yes with Right, and toggles with Up/Down; the NSH chooser clamps:
Up or Left selects No, Down or Right selects Yes. Return elsewhere goes back,
and neither host registers Exit, leaving it and held Return/Exit to the TV. The
[certification
checklist](https://developer.samsung.com/smarttv/develop/development-checklist/common.html)
item CO-US-05 describes an immediate close for the same home-screen press, so
Seller Office acceptance of the confirmation is an open question. If Samsung
rejects it, both hosts change together.

A system IME or popup is separate from the app-rendered exit question. App
termination on Exit over Patina's question is an implementation inference
pending TV observation. CO-US-04's popup-closing rule has no stated exclusion
for app popups, so this also needs owner/Samsung clarification.

### Pending device checks

These Back and exit checks (#277, tracked in
[#331](https://github.com/teng-lin/patina-iptv/issues/331)) need physical
hardware; headless and emulator results do not count.

| Target | Required observations |
| --- | --- |
| Android phone, API 34 or later | At the guide rail, the first-run welcome and library recovery with details closed, a committed edge gesture shows the predictive home preview and returns home, and a canceled one keeps Patina on the same screen. Forms and open recovery details still consume the first Back. |
| Both Tizen hosts | Return shows the question in the TV's language with No focused, at the guide rail and at first-launch setup (bridge) or the first-run welcome and recovery (NSH host). OK on No and Back dismiss it; Yes exits. On the NSH host, open recovery details consumes Return first. |
| Both Tizen hosts, Exit | Exit leaves through the system from an app page. Record what it does over Patina's question, and that it dismisses the TV's own keyboard or popup before the app ends. |
| Both Tizen hosts, held Return/Exit | The firmware ends the app without needing Yes, from an app page and from the open question. |
| Tizen 5.5 bridge | In a non-English TV language, the Worker loads the packaged `i18n/<tag>.tsv` and the question uses it. |
| Seller Office | At the first submission, confirm that the home-screen confirmation is accepted (CO-US-05). |

Use synthetic playlists and a dedicated test application or profile, never a
household one. Record physical and emulator runs separately, with the commit,
NSH revision, package SHA-256, device, OS or firmware, input mode, language and
observations. The pinned NSH Tizen renderer does not carry the app's
accessibility metadata into its page DOM, so check popup operation and Voice
Guide speech separately.

## Layout classes

One core policy picks the arrangement from the available logical window space,
the text size and the host's capabilities. Breakpoints come from what the content
needs at the current text size, not from the device type.

| Class | Arrangement |
|---|---|
| Expanded | Guide beside the player (preview layout), or VOD list beside details |
| Medium | Panes side by side where both stay usable; otherwise the compact arrangement |
| Compact | One pane at a time; the other is one action away |

- Below the space a pane needs, the layout collapses rather than shrinking text
  or touch targets further.
- Remote focus treatment does not change with window size.
- **Now:** the class policy (`iptv_presentation::layout::Class`): Compact below
  620 points and Expanded from 960, each divided by the text factor (guide scale
  times text size, the app's own times the system's: Android font scale or iOS
  Dynamic Type). Search's two-row header uses Compact.
- **Now:** the guide scales from 1.0 to 0.8 with the desktop window
  (`crates/iptv-app/src/kit/scale.rs`). Tablets keep full sizes in every window
  (Android screens; iPad full screen in either orientation, Split View, Slide Over
  and Stage Manager). A phone draws the guide at 0.7 (rows 50 → 35 dp, type
  20 → 14), with the system font scale still on top. For now a phone (smallest
  width under 600 dp, not a TV) runs in landscape (#168), either side as the
  rotation lock allows, until portrait has its own design. Tablets, an unfolded
  phone and TVs rotate freely.
- **Now:** the guide's header (`iptv_app::layout::GuideHeader`) puts its date and
  time ruler under the preview and focused program in Compact, and in Medium where
  the program needs more room. The preview stays top left and shrinks to leave
  that row room, never below 64 points tall (keeping the corner ‹ inside it). A
  short window instead grows the header, for the preview and the focused program's
  title and times (wrapped as the column makes them), up to 55% of its height (70%
  at accessibility text sizes) so the rail and guide keep room; shorter still
  squeezes them, the description giving way first. Times wrap onto a second line
  rather than clip, parts wider than the column wrap within it, the title stays on
  one truncated line and the date stays within the channel column. Guide rows take
  the full width below the header in every class.
- **Now:** a Compact window at least 1.6 times as tall as wide (upright phone,
  narrow portrait Split View) stacks the header: preview across the width inside
  its margins, then the focused program, then the ruler. It keeps the same 55%
  (70%) limit. Where it would pass that, or below the 1.6 ratio (a 400 × 600
  desktop window is 1.5; a phone on its side), the preview stays beside the
  program.
- **Now:** the player's channel list keeps its columns inside the window
  (`iptv_app::layout::ListPanel`): the groups column first, then the list in what
  is left. In the preview layout the video goes beside them only where it keeps
  160 points of width; a narrower window (an upright phone) leaves it full screen
  under the list, as the overlay layout does.
- **Now:** in Movies and Shows, details replace the list in every class and the
  poster grid reflows its columns. In Compact, an empty catalog (no titles,
  nothing filtered) is a pane of its own: its message and action take the width
  beside the rail (Close and the destinations), which stays narrow even with the
  remote on it, and the categories, with nothing to filter, give way
  (`iptv_presentation::vod::Screen::one_pane`).
- **Planned:** collapsing the categories beside the posters in Compact; until then
  they share the width with the poster grid.
- **Now:** the play bar keeps its three groups on one row where all buttons fit. A
  phone uses 44-point buttons and 12-point side margins, so on its side (740
  points across and wider) it keeps one row, with the quality and channel name on
  a line above. Narrower windows (iPad Split View or Slide Over, narrow desktop)
  wrap the right-hand group onto rows under the transport so no button is cut off.
  Choosers over the video keep clear of the bar's laid-out height, never exceed
  their room, and their lists scroll. The guide's corner ‹ keeps clear of the
  leading safe area (an iPad window's controls).
- **Planned:** a list beside its details in Expanded, and collapsing the player's
  panels, the rail and the groups column by class.

## What a presentation change keeps

**Planned** as a whole, with the current exceptions below. Collapsing or
expanding panes, resizing, rotating, entering split screen and switching input
keep:

- the selected content (channel, program, title, season, episode) by identity,
  never by row index;
- filters, sort and the scroll anchor;
- the playing stream;
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
  window (side by side too), display scale and text size changes, and Back
  returns to the same channel (`tests/app/desktop_workflows.rs`). Only the
  layout changes: the guide's header and Movies and Shows collapse by
  [class](#layout-classes).
- **Now:** layout changes keep the persistent `tv.player` surface: the player view
  is resized and moved, never hidden, detached or re-parented, and no other stream
  opens.
- **Now:** the first key after touch can move focus to a visible row (see
  [Input modes](#input-modes)). With `guide.autoplay` on, focus that lands on a
  live program after a pane closes can tune it.

## Which player is heard

[Platform support](platform-support.md#feature-availability) says where multiview is
offered and when an open session collapses to the heard tile.

Exactly one player is heard at a time, and the viewer chooses it; focus
never moves the sound (#135). The state is the core's
(`iptv_presentation::heard`, kept by `iptv_presentation::multiview` and
`iptv_presentation::playback_windows`), by the surface's identity.

- **Now:** in multiview, focus moves (arrows, taps, a tile's menu, changing its
  channel, enlarging or reducing it) never change what is heard. Every unheard
  tile's menu offers **Listen to this** (OK, tap or click), and the pointer chrome
  also gives those tiles a speaker button. With two or more tiles the heard tile
  shows a speaker beside its channel and the others a muted one; a screen reader
  hears "· muted" after a muted tile's name.
- **Now:** entering multiview keeps the sound on the channel that was playing.
  Adding a tile does not take it, and changing the heard tile's channel keeps it
  there. Removing the heard tile (its menu, its ✕, or its channel leaving the
  guide) gives the sound to the previously heard tile still present, else the tile
  on the full-screen player, else the first.
- **Now:** a full-screen detour plays the watched tile aloud; back on the grid the
  heard tile is heard again. Enlarging, reducing, reflowing and resizing keep the
  heard tile and open no stream.
- **Now:** the [interruption policy](#interruptions-and-the-background) pauses and
  resumes the heard player, whichever it is.
- **Now:** the optional **Sound follows the selected tile** (Settings, Playback,
  under Multiview; off by default; reached like any settings row) is for viewers
  who prefer the focused tile heard (#179). When on, each viewer focus move in
  multiview (arrows, a tap or click on another tile, a tile added, or picking a
  channel already on another tile) makes that tile heard, as **Listen to this**
  would, moving the speaker badge. Only a focus that moves takes the sound: a
  tile's menu, changing the focused tile's channel and an arrow at the grid's edge
  move nothing, and **Listen to this** still works. Detours, enlarging or reducing
  (also of another tile, which takes focus) and removing keep the rules above;
  focus they leave takes the sound only at the next focus move. Desktop player
  windows and picture in picture keep the explicit rule. The setting applies from
  the next focus move, even in an open multiview.

### Several players

**Open in new window** in a guide channel's menu (Linux and macOS) opens a player
window beside the main one; they all follow the one-heard rule.

- **Now: one player has the sound** (#135). A new player window opens
  muted, and opening one never takes the sound. Its title says so ("Music
  05 · muted"); the title is the channel's name alone in the window that
  has the sound. A compact window keeps the title "Picture-in-Picture"
  (window managers' rules match it), so it, and the main player's
  picture-in-picture window, always show a speaker or a muted speaker in
  the bottom left corner while other players are open. The main player
  shows its sound in its play bar's Mute control.
- **Now: Listen to this.** Every player that is not heard offers **Listen
  to this**: in its play bar beside Mute (the main player's too, while a
  window has the sound), and among a compact window's controls (bottom
  right). Choosing it moves the sound there, unmuted, and mutes every other
  player. A window's own Mute or volume, and the main player's Mute, Ctrl+M
  or volume, move it as before. Clicking into a window, moving the keyboard
  focus, resizing, full screen and compact never move the sound.
- **Now:** closing the window that has the sound gives it back to the
  player heard before it that is still open, else to the main player. A
  main player whose own Mute is on stays muted: only its Mute or Listen to
  this turns it on again.
- **Now:** each window shows its own channel and keeps its own pause,
  volume, audio delay and channel list. A window's compact
  picture-in-picture is that window made small and floating; its player and
  stream stay the same.

## Closing

One contract on the desktops, with or without recordings. Closing the main window
also closes every player window when recordings keep the app running, as when
quitting.

| Action | Result |
|---|---|
| A player window's ✕, Stop, G or its "TV guide" | That window closes and its stream stops; the other players play on |
| Escape in a player window | Leaves compact, else closes its dialog, else leaves the operating system's full screen, else closes the window |
| A compact window's ⇱, Escape or double click | Back to its normal window, still playing |
| A compact window's ✕ | That window closes and its stream stops |
| The main window's picture-in-picture: ⇱, Escape or double click | The video comes back to the main window, full screen |
| The main window's picture-in-picture: ✕ | Playback stops |
| Minimizing the main window | Everything keeps playing |
| Closing the main window, no recording running | Patina quits: every window closes |
| Closing the main window while a recording runs, on macOS | The main window hides and **every** player stops: main, multiview, picture-in-picture and player windows. The recorder runs on with the Dock icon; opening Patina again shows the main window. Nothing plays meanwhile, and guide autoplay waits until the viewer moves to another channel |
| Closing the main window while a recording runs, on Windows and Linux | Patina asks "Patina is recording": **Stop recording and quit** ends each recording as Stop does (what it holds is kept) and quits; **Keep recording** or Back leaves the window and everything in it as it was. No recorder runs without a window |
| Quit (the menu's Quit or the platform's quit key) while recordings run | The same question first, the window shown again if it was hidden |
| The system ending the session | Recordings stop with the recorder; what they hold is kept. The next start resumes one whose program is still on and finishes the others as partial recordings |

While a capture runs, Patina keeps the computer from idle sleep without root:
an IOKit `PreventUserIdleSystemSleep` assertion on macOS, a logind
`sleep:idle` inhibitor (or the desktop portal's Inhibit request) on Linux,
`SetThreadExecutionState` on Windows. It is released when no capture runs
(`host/native/keep_awake.rs`). If it cannot be taken, Record says to keep the
computer awake. Closing a laptop's lid still sleeps (logind ignores
inhibitors for the lid by default); on Linux, a suspend chosen from the
system menu may first say that Patina is recording.

- **Now:** all rows (`crates/iptv-app/src/shell/desktop.rs`,
  `shell/desktop/quit_confirm.rs`, `playback/player_windows.rs`,
  `playback/pip_window.rs`). The headless scenarios keep the macOS close
  behavior unless `IPTV_TEST_CLOSE_ASKS` is set
  (`desktop_workflows.rs`).

## Displays and hosts

- **Now (macOS, Windows):** when a display is removed, displays are
  arranged again, a window first shows on a saved place whose display is no
  longer there, or a window comes back from minimized or full screen, a
  Patina window
  whose title bar can no longer be grabbed moves onto the nearest remaining
  display, at its size or smaller
  (`iptv_app::shell::desktop::display_bounds::reachable`). A window the viewer can
  still grab is never moved. Moving a window plays and stops nothing.

| Host | Window positions | Compact window above others | After a display goes away | Fallback |
|---|---|---|---|---|
| macOS | Read and set | Floating level | Patina moves the main window, its picture-in-picture window and player windows when unreachable (above) | — |
| Windows | Read and set for the main window and its picture-in-picture window | Floating level requested for the picture-in-picture window (not checked on Windows) | Patina moves the main window and its picture-in-picture window | Separate player windows (**Open in new window**) are offered on Linux and macOS only |
| Linux, X11 | Not available: GTK 4 neither reports nor sets positions | Left to the window manager | The window manager keeps windows on screen | The compact window is titled "Picture-in-Picture", so window-manager rules written for browsers' picture-in-picture apply |
| Linux, Wayland (Hyprland) | Not through GTK; Patina places a window only when it first becomes compact, through Hyprland's IPC, and never moves one the viewer arranged | Hyprland rules that match the title | The compositor moves windows off removed outputs | As X11 |
| Linux, other Wayland compositors | Not available | Compositor policy | The compositor | As X11 |

- **Planned:** reporting these per-host differences as capabilities (#70).

## Cast live TV

On Linux, macOS, or Windows, play a live HLS channel and select **Cast to TV** in
the playback bar. Select your Chromecast. **Play here** brings playback back
to the desktop. For AirPlay on macOS, Linux and Windows, see
[sending video](platform-support.md#sending-video).

The receiver fetches the video over an HTTP connection back to Patina. If the
receiver appears in the picker but Patina reports that the TV did not request
the stream, allow the inbound TCP port named in that message from the receiver's
IP. The default is TCP 49152. Check that Wi-Fi client isolation allows devices
on the network to reach the desktop.

For example, on Linux with UFW, replace the example IP with your receiver's
address:

```sh
sudo ufw allow from 192.0.2.10 to any port 49152 proto tcp
```

For AirPlay on Linux (enabled in **Settings → General → AirPlay casting**), also
allow the UDP timing port (49153 by default, or `PATINA_AIRPLAY_TIMING_PORT`):

```sh
sudo ufw allow from 192.0.2.10 to any port 49153 proto udp
```

If your firewall already permits a different port from the receiver, quit
Patina and launch it with that port. For example, if TCP 53317 is available
and allowed:

```sh
PATINA_CAST_RELAY_PORT=53317 patina
```

Use the same environment variable with your desktop launcher if you want it
applied on subsequent launches. See [Casting on the local
network](security.md#casting-on-the-local-network) for what the relay shares
with the receiver.

## Text entry

- **Now:** while a text field has the focus (search, a playlist address, a PIN),
  the keys typed go to it: a letter, a digit or Space that is a player shortcut
  elsewhere neither pauses, mutes nor zaps. On a desktop that includes letters,
  digits, Space, /, =, -, [, ], the arrows, Home, Backspace, selection and the
  system's copy, cut and paste, and Ctrl+↑/↓; no shortcut fires. A Ctrl shortcut
  the field does not use, such as Ctrl+M, still acts on the focused window's
  player. Typing never zaps, closes or pauses anything.
- **Now:** a software keyboard keeps the field above it, in landscape, portrait
  and split screen, and its dismissal ends the editing
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
| Home with picture in picture on (phones and tablets, from full-screen playback only; never a TV) | Plays on in the window, on the same player and stream |
| The picture-in-picture window closed | Stops as above |

In multiview the policy pauses and resumes the heard tile (the one the
viewer chose with Listen to this, whatever has the focus; the others are
muted), as the tile's own pause.

| During a call | Playback |
|---|---|
| The viewer chooses Listen to this on another multiview tile | Stays silent until the call ends (Android: every player is silenced while the focus is lost for a while); the app pauses nothing more, and the call's end resumes nothing: the heard tile is the viewer's choice. Only moving the focus changes nothing |
| The viewer picks a channel a guard refuses (blocked, locked) | Nothing changes: the pause is still the call's, and its end resumes |
| Keys and controls that are no playback action (Info, menus, highlights) | Nothing changes |

- **Now (Android):** all the activity's players share one audio focus
  (`AudioFocusHost.kt`, rules in `AudioFocusState.kt`). It is held while a player
  plays aloud (source buffering or ready, set to play, unmuted; not ended or
  failed); never taken back from another app until a player plays again; kept
  through a call while a stream is open, and afterwards until the app has decided
  (2 s at most); and given up when every stream is stopped, closed or in the
  background. Changes reach the app as the player's `audio` event. A stream the
  viewer stopped stays stopped whatever the focus does, and resuming opens no
  second provider connection (`tests/app/tablet_journey.rs`). Background stop and
  return, and picture in picture, are in `PlayerComponent.kt` and `PipHost.kt`.
- **Now:** Record now says, as a capture starts, what will stop it on this
  host: turning an Android TV off under Low Power Standby, Android's
  six-hour background limit, a metered network, or a desktop that could not
  be kept awake (`iptv_presentation::capabilities::record_now_notices`).
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
  under the same policy.

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
| No source can provide movies or series (no playlist yet; once every enabled playlist is M3U, Movies and Shows are not offered) | Movies and shows come from Xtream Codes and Stalker playlists; M3U has live channels only | Add playlist |

Where the remote goes:

- The action sits where the titles would be: Right from the categories, then OK.
  If it moves to another playlist while the remote is on it, the remote returns to
  the categories, so OK never acts on a playlist the screen did not show.
- Medium or Expanded with no action (import running, or titles only elsewhere):
  the remote never rests on the empty place. An action going away (Refresh now
  started) sends it to the categories; Right from them reaches Search and Sort,
  Down from those stays, Left from Search returns to the categories, OK on a
  category stays, and closing Search keeps the remote on it. Search closed while
  its list is still being read keeps the remote on Search until the list arrives,
  then moves it to the titles or action if any. A remote sent off the action's
  place this way stays off a later action, even if the window becomes Compact,
  until it moves there itself. Once an action shows, Right and Down reach it as
  before.
- [Compact](#layout-classes): the message and action take the width and the
  categories give way. A remote or keyboard that was on the categories goes to the
  action, or to the rail when there is none (import running) or it was just sent
  off another playlist's action; Left reaches the rail and Right returns. An action
  that moves to another playlist or goes away (refresh started) sends it to the
  rail; if the window widens or text shrinks, the rail stays on screen with the
  remote on it. Without an action, Right from the rail reaches Search and Sort,
  and closing Search keeps the remote there.
- In every window, a remote sent off an action, or on Retry when the action moves,
  stays off the new one through a failed read and its Retry, until it moves there
  itself (Right, or OK on a category).

The action stays inside a 400×600 window at Medium and Extra large text, also
with the test pseudolocale (`tests/app/vod_empty_compact.rs`). A search or filter
that finds nothing says "No titles found." with no action. Continue watching and
Recently added keep their own empty text. The "Movies and series" switch keeps no
own value when it matches the global setting, and a deleted playlist's value goes
with it. Imports are never switched on automatically, and M3U channels never
appear as movies or series.

## Admission and capability

Every play action, from any input or layout, goes through the same checks.
Unavailable actions say why, using the host's reported capabilities
([platform support](platform-support.md)), not the platform's name.

- **Now:** streamed playback goes through the parental lock and provider
  connection admission. A recording plays from its local file: it has its own
  parental check and needs no provider connection.

## Verification

- The repository's required checks in [AGENTS.md](../AGENTS.md), including
  `cargo test -p iptv-core -p iptv-presentation` for Domain and interaction rules,
  the wasm builds of both crates and `cargo build -p iptv-app`.
- Which player is heard: presentation tests in `iptv_presentation::heard`
  (including hand-back), `iptv_presentation::multiview` (`heard_tests`, with and
  without sound following the selected tile) and
  `iptv_presentation::playback_windows`; app scenarios `multiview_heard` and, for
  windows, `desktop_workflows`.
- Choosing, staying and returning: app scenarios `personalization_overlay`,
  `tv_shared_controls`, `pip_control`, `cov_nav` and `cov_nav_touch`.
- Remote keys: `iptv-nav` tests `hold`, `press` and `keymap`
  (`long_ok_opens_controls_once_even_after_the_context_changes`); app scenarios
  with an Android remote's raw Down/Repeat/Up phases (`use_remote_key_phases`),
  `tv_remote_journey` (holds by repeat and timer, surfaces changing mid-press, PIN
  prompts, stream-open counts on one persistent player, extra large text, and a
  desktop keyboard's PIN repeats).
- TV idiom: app scenarios `tv_shared_controls`, `tv_player`, `tv_replay` (replay
  seek and track choice on one player), `accessibility_visibility` (covered and
  inactive controls out of reach, PIN and modals at the largest text, identity
  after reordering) and the pointer regressions.
- Tablet gestures, the touch journey, rotation, split screen, large text,
  text entry and interruptions: `tablet_gestures` and `tablet_journey`.
- iPad portrait, resizable windows, Dynamic Type and another app beside:
  `ipad_windows`.
- Pointer, keyboard and windows on desktops: headless app scenarios
  `desktop_workflows`, `guide_player_windows` (#60's player windows),
  `pip_control`, `pane_collapse`, `guide_nav` (the wheel through time) and
  `pointer`; app unit tests in `iptv_app::shell::desktop::display_bounds`,
  `iptv_app::shell::desktop` (window recovery gating, pointer travel) and
  `iptv_app::playback::player_windows` (session admission, window placement).
- Native behavior on Android TV and tablets: the
  [disposable Android validation](testing.md#disposable-android-validation) on `tv36` and `tab36`.
  TV validation sends real key phases with generated playlists and streams.
  Record emulator and physical-device evidence separately, with the build, OS,
  input and viewport, using synthetic data only. Headless layout and keymap tests
  do not establish physical-remote or screen-reader speech behavior.
