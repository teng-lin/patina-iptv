# TV playback controls

The rules shared with touch and desktop (choosing, returning, layout changes)
are in the [interaction contract](interaction.md). This page is the remote's
key table: what each key does on each surface, a tap and a hold. It describes
the code as it is (`iptv_nav::Keymap::defaults`, `crates/iptv-app/src/input.rs`,
`player_keys.rs`, `play_bar.rs`); a viewer can rebind keys in Settings → Remote
(`remote_editor.rs`), which this table does not cover.

## Taps and holds

- **Hold** means the key stays down for `remote.long_press` (400 ms by
  default). Only an Android remote reports holds; a desktop keyboard has
  Shift and Menu (`M`) stand-ins instead.
- A key with a hold action on the current surface waits: released earlier,
  it is a tap and acts on release; held to the threshold, its hold action
  acts once, on an auto-repeat or on a timer for remotes that send none. The
  release after a hold does nothing, and neither do the repeats.
- A key without a hold action acts at once and again on each repeat, so a
  held arrow keeps moving. Menu is the exception, on a remote and as `M`
  on a keyboard: held, it acts once instead of opening and closing a menu
  or the play bar with every repeat.
- A hold decided by an auto-repeat is the hold's one action, exactly as one
  decided by the timer.
- A waiting press belongs to the surface it went down on: the keymap's
  context and what owns input within it (the info card and the control
  strip share a context but are different surfaces). If the surface at the
  moment the press is decided differs from the one where it began (a touch
  played a channel or opened More controls, a prompt opened, the guide went
  full screen), the press is dropped: neither its tap nor its hold action
  runs (`iptv_nav::HeldPresses`; the log says "dropped: the surface
  changed"). A surface that changes and comes back before the decision
  counts as unchanged, and the press acts there.
  A dropped Back also takes Android's own Back on its release with it.
  A waiting press that another key's press replaces (Menu pressed while
  OK is held on the play bar) stays replaced until it is released or
  pressed afresh: its later repeats start no new press where it did not
  go down.
- A PIN prompt or the remote editor takes the keys it gets: a press pending
  from before is cut short and stays so until that key is released or
  pressed afresh (its repeats, release or timer do nothing, even after the
  prompt closes), and a key the prompt took does nothing more while held
  (the OK that submitted a PIN starts no hold on the video). This holds
  however the prompt opens and closes, by touch too, with no key reaching
  it. A Back held from before the prompt does not dismiss it on release,
  whatever other key is pressed meanwhile. A fresh Back on a PIN prompt
  closes it once, through Android's Back; if touch closes that prompt
  while Back is down, Back's repeats do nothing and its release takes no
  step back from what the prompt covered.
- On a desktop keyboard, a key a PIN prompt took (the Enter that closed
  it) does not repeat until it is pressed afresh, so holding it does not
  act on what the prompt covered; a held arrow or digit in the prompt
  acts once.
- A key going down keeps the info card and the play bar on screen until
  the press is decided, even when their own timeout is shorter than the
  hold, so a press never finds them gone on release; input meanwhile
  (a zap's banner) restarts their own timeout without shortening that.
- In a replay the seek arrows have no hold: held, they repeat the seek.
- Long Back's release takes no step back.

"–" below means the key does nothing there. Keys not listed for a surface
fall back to the global row: Search opens search, Red toggles the favorite,
digits enter a channel number, Play/Pause, Play and Pause play and pause,
Stop stops playback, ⏮ (Previous) is the previous channel and ⏭ (Next) goes
back to live.

## Guide

| Key | Tap | Hold |
|---|---|---|
| Up / Down | Move between channels, wrapping at the first and last rows. The groups column wraps between its first and last entries. | Repeats |
| Left | Earlier programs, then the groups column, then the rail | The groups column |
| Right | Later programs | Repeats |
| OK | Choose: tunes a live channel in the TV Guide preview; the playing channel returns to full screen; a past program with replay replays; any other program opens its details | The program and channel menu |
| Menu | The program and channel menu; on the groups column, the group's options | – |
| Back | Hint card, then back to the programs on air, then groups, then rail, then leave (see [Back](#back)) | Back to the video, when something plays |
| Ch+ / Ch− and Page Up / Down | A page of channels | Repeats |
| Record | Records or schedules the focused program | – |
| Guide | – (already the guide) | – |
| Info | – | – |

Moving the focus never tunes a channel unless **Autoplay** (`guide.autoplay`)
is on; it then tunes the focused live channel after `guide.switch_delay`, and
moving on before then cancels it.

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

## Full-screen live video

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
`guide.switch_delay`, so quick presses skip channels. Browsing the channel
list never tunes unless autoplay is on.

### Info card and its action cards

A custom remote binding can open the info card with action cards. The cards capture Left, Right and OK while they show.

| Key | Tap | Hold |
|---|---|---|
| Left / Right | Move between the action cards (Guide, History, Channels, Favorite, Multiview, More controls) | Repeats |
| OK | Presses the focused action card; with none focused, hides the card | The play bar |
| Up / Down | Channel up / down, the card following | Repeats |
| Menu | The play bar | – |
| Back | Hides the card | Stops playback |

The card hides by itself after `player.controls_panel_timeout`; a key keeps
it up.

## Replay and seekable video on demand

As full-screen live, except:

| Key | Tap | Hold |
|---|---|---|
| Left / Right (`catchup.seek_keys`: Left/Right, the default) | Choose a seek one time-bar step back or on, shown on the card's bar; it starts after a short pause, or at once on OK | Repeats the step, faster (no hold action) |
| Down / Up (`catchup.seek_keys`: Down/Up) | As Left / Right above, instead of zapping | As above |
| OK | Starts a pending seek at once or opens the play bar with Play/pause focused | Same as tap |
| Rewind / Fast-forward | Seek back / on | Repeats |
| ⏭ (Next) | Back to live (a replay) | – |

A seek and a track choice keep the same player. A replay's seek opens the
archive at the new time; a track choice opens no stream, and nothing else
does either.

## Play bar

The same play bar appears on tablet and desktop. On TV it has remote focus. It captures the
arrows and OK while it shows, so they never zap or seek behind it.

| Key | Tap | Hold |
|---|---|---|
| Left / Right | Move between the enabled controls (live skips Rewind and Fast-forward); on the timeline, seek | Repeats |
| Down | The timeline (replay or seekable video on demand); elsewhere stays | – |
| Up | Back to Play/pause | – |
| OK | Presses the focused control; on the timeline, starts the seek and returns to Play/pause | Focus back on Play/pause; the repeats press nothing |
| Menu | Hides the bar | – |
| Back | Closes a chooser the bar opened, then the bar | Stops playback |

The bar hides by itself after three seconds without use; a chooser it
opened keeps it up. The remote skips unavailable controls; the advanced
strip names the reason where one applies (for example "Picture in picture ·
Play a channel first").

## Choosers, menus and dialogs

| Surface | Up / Down | OK | Left / Right | Back or Menu |
|---|---|---|---|---|
| Chooser (picture size, tracks, sleep timer, audio delay, history) | Move | Picks and closes; focus returns to the control that opened it | – | Closes; focus returns to that control |
| Program and channel menu | Move | Performs the item | – | Closes; focus returns to the program or channel |
| Program details | – | Records the program | – | Closes |
| Channel list over the video | Move | Plays the focused channel | Left: the groups; Right: back to the channels | Back: from the groups to the channels, then closes; Menu: the channel's or group's menu (also Hold OK) |
| PIN prompt | Move between digits and actions | Presses the focused action | – | Back closes; the content it guarded is not played |
| Settings and other screens | Move | Activates | Move between columns | Back returns to where Settings opened, on the same channel |

Ch+ / Ch− in the channel list zap without closing it.

## Back

Back always dismisses the nearest surface, one step per press
(`go_back` in `crates/iptv-app/src/lib.rs`):

1. A channel number being typed.
2. A PIN prompt, then a screen opened over the app (wizard, Movies and
   Shows, Recordings, Settings and its sub-screens).
3. A chooser, then a menu or program details.
4. Search, the channel list over the video, the multiview picker.
5. Over video: the info card or the play bar.
6. Full-screen video: back to where it was chosen (the guide on the playing
   channel, Recordings for a recording, the multiview grid for a tile). The
   stream keeps playing in the mini-player or behind the guide.
7. In the guide over the video, back to the video.
8. In the guide: the hint card, the programs on air, the groups column,
   the rail, then leave.

At the top level (the rail, with nothing open, nothing playing behind
the overlay layout's guide and no channel number being typed) the app
registers no Back handler, so Back follows the platform's convention
(#277): Android TV returns to the launcher in one press, and Android
phones and tablets show the predictive back animation (on Android 15 and
later; on Android 14 only with the developer option "Predictive back
animations"). No second press is asked for. On a desktop, Escape there
does nothing; the window closes the operating system's way. The former `general.confirm_exit` and
`general.confirm_exit_window` settings are no longer offered, and stored
values are ignored.

Closing a chooser, menu, PIN prompt or settings screen restores focus by
content identity (the control, program or channel), not by row position,
also after the list was refreshed or reordered.

## Returning to playback

- From the guide, OK on the mini-player, OK on the playing channel, Back in
  the guide over the video and a hold of Back all return the playing stream
  to full screen without opening it again.
- Browsing (moving focus in the guide or the channel list) never retunes
  unless autoplay is on.
- Layout changes retain the persistent `tv.player` surface; they do not
  hide, detach, reparent or open another stream.

## Checks

Deterministic, run by CI:

- `iptv-nav`: `hold` (a waiting press dropped when its surface changes;
  long OK once through repeats; rapid taps; a press another key replaced
  does not restart from its repeats), `press` and `keymap`
  (`long_ok_opens_controls_once_even_after_the_context_changes`).
- App scenarios with an Android remote's raw Down/Repeat/Up phases
  (`use_remote_key_phases`): `tv_remote_journey` (long OK by repeats and by
  timer, rapid taps, OK held as the info card times out, OK held while a tap
  plays another channel, the info card giving way to the strip mid-hold,
  PIN prompts that take or swallow keys, Back dropped mid-hold, a card
  timeout shorter than the hold, a hold decided by a repeat on the play
  bar, a held Menu, the full journey with stream-open counts and one
  persistent player, extra large text on the play bar; a PIN prompt opened
  and closed by touch around a held OK or Back, a Back pressed in a prompt
  that touch closes, another key tapped while Back is held, a zap committing
  mid-hold over the card, Menu replacing a held OK on the play bar), and a
  desktop keyboard's Enter that closed a PIN prompt holding its repeats.
- App scenarios with the TV idiom: `tv_shared_controls`, `tv_player`,
  `tv_replay` (replay seek and track choice by remote on one player),
  `accessibility_visibility` (covered and inactive controls out of reach,
  PIN and modals at the largest text, identity after reordering) and the
  pointer regressions.

Native TV validation sends real key phases on a disposable `tv36` emulator
with generated playlists and streams; see
[issue #87 TV journey](https://github.com/teng-lin/patina-iptv/blob/cb6b863b4635f40367ef5dce804d8fefe6ff0819/docs/validation/issue-87-tv-journey.md). Headless layout and
keymap tests do not establish physical-remote or screen-reader speech
behavior.

## Android journey evidence

On 2026-11-16 UTC, a disposable `tv36` (Android 16/API36, x86_64,
1920×1080 physical / 960×540 logical, D-pad, TalkBack disabled) imported two
generated channels from a local mock playlist. The reserved test stream names
exercise player state and layout; decoded video is not claimed.

Native accessibility trees showed the mini-player selected after Up from the
first guide row, then the same playing channel full screen after OK. A native
long OK reported Down with no action, a timer `ShowControls`, and Repeat with no
action after the context changed to Controls. The released key produced neither
`ShowPanels` nor `Select`. The shared bar exposed Play/pause selected; Right
skipped disabled live Fast-forward and selected Record. These are native
input/layout checks; persistent surface identity and stream-open counts are
asserted by the headless live/replay scenarios. Physical remotes and spoken
screen-reader output remain unverified. The #87 run is recorded in
[issue #87 TV journey](https://github.com/teng-lin/patina-iptv/blob/cb6b863b4635f40367ef5dce804d8fefe6ff0819/docs/validation/issue-87-tv-journey.md).
