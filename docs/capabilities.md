# Feature availability

The portable [capability model](../crates/iptv-core/src/capabilities.rs) separates
host facilities, live player reports and recording wake registration. The
[presentation capability rules](../crates/iptv-presentation/src/capabilities.rs)
decide when to offer PiP and multiview. The
[shell adapter](../crates/iptv-app/src/capabilities.rs) chooses the host profile;
native services report what the operating system actually accepted.

| Host profile | Player surfaces | Recorder can outlive UI | Login registration | Alarm permission control |
| --- | --- | --- | --- | --- |
| Android TV/tablet | 2 | Yes | No | Yes |
| Linux | 4 | Yes | Yes | No |
| macOS | 4 | Yes | Yes | No |
| Windows preview | 4 | Yes | Yes | No |
| iPad | 4 | No | No | No |
| Unknown | 1 | No | No | No |

## Device signals

The operating system alone cannot tell an Android phone from a tablet or a
TV, so gates also take three signals that can change while the app runs
([portable model](../crates/iptv-core/src/capabilities.rs),
[`Device`](../crates/iptv-presentation/src/capabilities.rs),
[shell adapter](../crates/iptv-app/src/device.rs)):

| Signal | Source | Gates |
| --- | --- | --- |
| Form factor: phone, tablet, TV or desktop | Android: the TV UI mode or the leanback feature, otherwise the physical display's smallest width (under 600 dp is a phone), never the window's, so a tablet in split screen stays a tablet ([`DeviceComponent.kt`](../hosts/android/app/src/main/kotlin/com/hantekllc/iptvrs/host/DeviceComponent.kt)). iPad: the system's idiom. Desktops: the build. Tizen: a TV. | Which capabilities exist |
| Window size class: compact, medium or expanded | The [layout class](../crates/iptv-presentation/src/layout.rs) of the current window at the guide's scale and text size | Layouts |
| Connected input: a hardware keyboard, a D-pad | Android: `Configuration.keyboard` and `navigation`, unless hidden. Desktops: a keyboard. Tizen: its remote's D-pad. iPad: none yet (`GCKeyboard` is not read). | Keyboard and remote features |

Android reports the form factor and the input when the app starts and on every
configuration change; before that report an Android build counts as a tablet
with neither input. The settings offer check (`settings::offer`), the control
strip and multiview's offer read the signals as they are now, so a
change takes effect without a restart. A control that leaves the strip while
focused passes the focus to its nearest neighbor. The Keyboard or Remote
settings, and guide autoplay with its delay, follow the form factor and the
connected input ([TV controls](tv-controls.md)); a settings page without rows
leaves the list while the screen stays open, and the page shown, or the one in
its place, keeps the focus. Multiview follows the form factor and the window
(below). The host profiles above still follow the platform alone.

Android also reports its video decoder capacity: the instances the hardware
H.264 decoder the player picks supports at once
(`MediaCodecInfo.CodecCapabilities`), or nothing without a hardware H.264
decoder, when Media3 decodes in software. The
report can only lower the platform's surface budget, so a device that cannot
run two hardware decoders at once loses multiview rather than gaining tiles.

Settings the signals decide (#281, `settings::offer`). The rewind and
fast-forward jumps (`catchup.forward_step`, `catchup.rewind_step`) are on the
Advanced page on a desktop or a TV; a phone or a tablet, which has no Advanced
page, keeps them with the replay jumps:

| Setting | Phone, tablet | Desktop | TV | Compact window |
| --- | --- | --- | --- | --- |
| `general.clock` | Hidden: the system's 24-hour setting decides | Shown | Shown | As the device |
| `appearance.ui_transparency`, `guide.browse_mode`, `guide.list_browse_mode` | Shown | Shown | Shown | Hidden: the guide, the Guide key over full-screen video included, shows its preview; the channel list puts the video beside it wherever it has room |
| `guide.fullscreen_timeout` | Hidden, never | Hidden, never | On the Advanced page | As the device |
| `guide.after_choosing` | Hidden: the guide over the video and the channel list always go full screen | Shown | Shown | As the device |
| `player.info_on_switch` | Hidden on Android unless a keyboard or a D-pad is connected; shown on an iPad, whose input is not reported yet | Shown | Shown | As the device |

Android's activity also reports its accessibility settings: the timeouts
`AccessibilityManager.getRecommendedTimeoutMillis` gives for controls and for
text, and whether Switch Access (or Samsung's Universal Switch) runs. Switch
Access is recognized heuristically, by the class names of the enabled
accessibility services (`SwitchAccessService`, `UniversalSwitchService`),
since no service property marks a switch service. The
player's panels and the play bar never hide sooner than the recommended
timeout, and stay until the viewer hides them while a screen reader (as NSH
reports it, on every host) or Switch Access runs. iPadOS's Switch Control is
not detected: NSH reports VoiceOver only.

Every row above stays in the settings catalog, so its stored value loads and
restores on any device. A hidden row's value is kept and applies again on a
device that offers it (a TV's Clock restored on a phone is ignored there, and
kept). The retired `guide.stay_on_guide` and
`guide.stay_on_list` are carried into `guide.after_choosing` when the settings
load, at startup and so after a restore, while that is not stored; the retired
`guide.back_returns_to_now` is ignored (always on).

These are configured limits and facilities, not certification of a device's
decoder performance. The profile tests exercise each row without needing that
OS. Android TV and tablet share the same surface budget; the native PiP report
also distinguishes a TV, which never has picture in picture on any build: its
host reports it unsupported and refuses to enter, by request or on Home.

PiP starts unknown until the player reports support; hosts without a native
implementation (the iPad, browser TVs) acknowledge it as unavailable during
setup. Where it is unavailable the control strip and the play bar leave the
control out, rather than show it disabled, and the setting
`player.pip_on_leave` is hidden. Its button and action
handler use the same current decision. A denied or changed report takes effect
without restarting the application. Desktop PiP presents the original player;
it never reserves another provider stream.

Multiview is offered by device and window size
([`multiview_offer`](../crates/iptv-presentation/src/capabilities.rs)), the same
way through the play bar, the control strip and the info card's action card:

| Host | Multiview |
| --- | --- |
| Android phone | Two tiles, under the play bar's More only, with the phone on its side (its window wider than tall). Left out upright and at the account's connection limit. No "Sound follows the selected tile" setting. |
| Android tablet | Two tiles in an expanded window; left out in a medium or compact one (split screen). |
| Android TV | Two tiles. At the account's connection limit it is shown disabled, and choosing it says why. |
| iPad | Left out until measured on a device (iPad draws every mpv frame on the main thread), so never in Slide Over or a compact window. |
| Desktop | Four tiles. |
| Tizen and other one-decoder hosts | Left out, never drawn disabled. |

The account's connection limit is reached when it allows one stream, the one
already playing full screen; elsewhere adding a stream past the limit asks
first. A metered network should also leave multiview out on a phone, but no host
reports one yet. When the window or the device stops offering multiview during a
session (a tablet's window going to split screen, a phone turned upright), the
session collapses to the tile being heard, which plays on full screen. When that
tile plays on the full-screen player (surface 0) it keeps its stream; a tile heard
on another surface plays on in surface 0, as Exit multiview does, since no player
view moves. "Sound follows the selected tile" is hidden on phones and wherever
multiview never exists; its stored value is kept, unused.

Multiview checks the main player, available decoder surfaces and extra player
windows at both presentation and action time. The provider's connection budget
and parental authorization remain separate checks at native admission; a
feature being available grants no permission to play content.

Recording capabilities report background-host ownership separately from the
last acknowledged wake registration: unknown, awake-only, exact, inexact or
denied. The runtime does not infer exact alarms from a platform name. Permission
changes are reflected when the host next acknowledges registration. Unchanged
scheduler inputs alone do not imply a fresh permission check. Opening the background
status command uses the current report. The recording screen refreshes its
notice through its existing reload loop.
Returning to the foreground requests a fresh acknowledgement, including after
leaving Android's alarm-permission settings. That refresh does not open a stream.
It uses the recorder's current schedule, including the earlier recurring guide
check deadline, so refreshing permission cannot postpone discovery of episodes.
During native startup a refresh returns without replacing the saved alarm;
the first loaded job/rule snapshot supplies the new schedule. Foreground checks update the
persistent recording notice without repeating its warning toast on every return.

Awake-only desktop capture can outlive the window while the host process and
computer remain running. iPad capture requires the app to remain open. Android
uses a foreground service and is subject to force-stop and OS execution limits.
Windows wake tasks remain experimental; a successful registration is not
physical-machine wake validation. An unknown report does not promise an
unattended start. See [architecture](architecture.md) and
[platform installation details](../README.md) for the supported distributions.

Validation combines portable profile/decision tests, I/O-free initial capability
inspection, native startup/restart tests and permission changes without reopening
credentials, headless control scenarios and native host smoke checks. Real-device
and emulator evidence must remain distinguishable.
