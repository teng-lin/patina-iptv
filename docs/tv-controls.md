# TV playback controls

Android TV uses the same playback bar as tablet and desktop. Hold OK to open it
with Play/pause focused. Menu opens or closes the bar. A short OK over unobstructed
video retains the info card behavior; release after a long OK performs no second
short action.

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
