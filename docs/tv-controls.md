# TV playback controls

The rules shared with touch and desktop (choosing, returning, layout changes)
are in the [interaction contract](interaction.md).

Android TV uses the same playback bar as tablet and desktop. Hold OK to open it
with Play/pause focused. Menu opens or closes the bar. A short OK over unobstructed
video retains the info card behavior; release after a long OK performs no second
short action. The live info card's More-controls action retains the advanced strip,
including sleep timer and audio-delay controls.

Left/Right moves through enabled controls. Live playback skips rewind/fast-forward;
Down enters the seek timeline during replay or seekable VOD, where Left/Right seeks.
Up returns to Play/pause. OK activates the focused control. Back closes a chooser,
then the bar, then returns to the guide. Controls use the same admission checks and
actions for pointer and remote input. Popup dismissal preserves control identity.

In the guide, Up from the first channel row focuses the playing mini-player.
Down or Back returns to the anchored channel; OK returns the existing playing
stream to full screen even when the guide focuses a different channel. A list
refresh restores the guide's content identity. Layout changes retain the persistent
`tv.player` surface; they do not hide, detach, reparent or open another stream.

Deterministic checks: core `play_bar`/`browse` tests; nav
`long_ok_opens_controls_once_even_after_the_context_changes`; app
`tv_shared_controls`, `tv_player`, `tv_replay`, `accessibility_visibility` and pointer
regressions. Native TV validation must additionally send actual Down/Repeat/Up
key phases (including a long OK release), inspect shared bar focus and verify
no accompanying short action, then exercise guide mini-player/full-screen return.
Use only disposable `tv36` and generated playlists/streams. Headless layout and
keymap tests do not establish physical-remote or screen-reader speech behavior.


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
screen-reader output remain unverified.
