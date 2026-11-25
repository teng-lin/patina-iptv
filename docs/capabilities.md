# Feature availability

The portable [capability model](../crates/iptv-core/src/capabilities.rs) separates
host facilities from live player reports. The
[presentation capability rules](../crates/iptv-presentation/src/capabilities.rs)
decide when to offer PiP and multiview. The
[shell adapter](../crates/iptv-app/src/capabilities.rs) chooses the host profile;
native services report what the operating system actually accepted.

| Host profile | Player surfaces | Recorder can outlive UI |
| --- | --- | --- |
| Android TV/tablet | 2 | Yes |
| Linux | 4 | Yes |
| macOS | 4 | Yes |
| Windows preview | 4 | Yes |
| iPad | 4 | No |
| Unknown | 1 | No |

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
| Connected input: a hardware keyboard, a D-pad | Android: connected keyboards and D-pads, including game controllers with hat axes. Android configuration supplies the fallback and reports hidden built-in devices. Desktops: a keyboard. Tizen: its remote's D-pad. iPad: the current `GCKeyboard` report, refreshed on attachment, detachment and activation. | Keyboard and remote features |

Android reports the form factor and input when the app starts and when the
configuration, display or connected devices change. Before the first report,
it counts as a tablet with neither input. A tablet keeps its scale in split
screen, even when compact layout rules apply. The settings, control strip,
play bar and its More list, guide menus, rail and multiview read the current
signals. Focus stays with its action as rows change and moves to a nearby
available action when the focused row leaves. An active recording keeps its
Stop action. The Keyboard or Remote
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
A lower decoder or form-factor budget collapses an open session to its heard
tile, even when the smaller budget still offers a new multiview session.

Settings the signals decide (#281, `settings::offer`). The rewind and
fast-forward jumps (`catchup.forward_step`, `catchup.rewind_step`) are on the
Advanced page on every device, including phones and tablets whose touch play
bar uses these jumps. The time-bar interval stays with the replay settings:

| Setting | Phone, tablet | Desktop | TV | Compact window |
| --- | --- | --- | --- | --- |
| `general.clock` | Hidden: the system's 24-hour setting decides | Shown | Shown | As the device |
| `appearance.ui_transparency`, `guide.browse_mode`, `guide.list_browse_mode` | Shown | Shown | Shown | Hidden: the guide, the Guide key over full-screen video included, shows its preview; the channel list puts the video beside it wherever it has room |
| `guide.fullscreen_timeout` | Hidden, never | Hidden, never | On the Advanced page | As the device |
| `guide.after_choosing` | Hidden: the guide over the video and the channel list always go full screen | Shown | Shown | As the device |
| `player.info_on_switch` | Hidden unless a keyboard or a D-pad is connected | Shown | Shown | As the device |

Android's activity also reports its accessibility settings: the timeouts
`AccessibilityManager.getRecommendedTimeoutMillis` gives for controls and for
text, and whether Switch Access (or Samsung's Universal Switch) runs. Switch
Access is recognized heuristically, by the class names of the enabled
accessibility services (`SwitchAccessService`, `UniversalSwitchService`),
since no service property marks a switch service. The
player's panels and the play bar never hide sooner than the recommended
timeout, and stay until the viewer hides them while a screen reader (as NSH
reports it, on every host), Switch Access or iPadOS Switch Control runs.
The iPad observer reads GCKeyboard and UIKit Switch Control state on mount,
follows their notifications, refreshes both when the app or scene becomes
active, and removes every observer on unmount.

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
first. Using the metered-network signal to gate phone multiview remains a
follow-up. When the window or the device stops offering multiview during a
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

## Record now and the Recordings library

Record now is offered only where the viewer stays with the capture and the
host can keep it running (#275;
[`record_now`, `recordings_library`](../crates/iptv-presentation/src/capabilities.rs),
[shell gates](../crates/iptv-app/src/capabilities.rs)):

| Host | Record now | Recordings library |
| --- | --- | --- |
| Android TV | Play bar, control strip, program menu and details, Record key | Always |
| Desktop | Same as Android TV; the computer is kept from idle sleep during capture | Always |
| Android phone and tablet | Under the play bar's More, control strip, program menu, Record key; not the details' button | Once a read of the library finds recordings (media or carried-over jobs) |
| iPad | Nowhere | Once a read of the library finds recordings |
| Tizen (bridge and NSH host) | Nowhere | Never |

Every entry point reads the same gate: the play bar, the control strip, the
program menu and details, the Record key (its default binding is removed where
Record now is absent, and the remote editor does not offer it), and Recordings
in the rail and the on-demand browser's navigation. A recording already running
or carried over can still be stopped or canceled from the program menu.

Before a capture starts the runtime checks the library's 32 GiB budget and the
free space for the requested length at 8 Mbit/s
([`check_space`](../crates/iptv-core/src/recording.rs)), then a free provider
connection; a refusal says why. As it starts, the viewer is told what will stop
it ([`record_now_notices`](../crates/iptv-presentation/src/capabilities.rs),
[`capture_limits.rs`](../crates/iptv-app/src/capture_limits.rs)):

| Host | Signal | Told at start | When it happens |
| --- | --- | --- | --- |
| Android TV | Low Power Standby on and Patina not exempt (`PowerManager`, API 33 and 34) | Turning the TV off stops the recording | The screen turning off ends the capture as a partial recording with the reason, and a notification says so |
| Android phone and tablet | `ConnectivityManager.isActiveNetworkMetered` | A metered network can use several GB an hour | — |
| Android (TV, phone and tablet), Android 15 and later | The data-sync service's six hours a day | Android stops background recording after six hours | `onTimeout` ends the capture as partial, notifies, and stops the service |
| Desktop | The idle-sleep inhibitor (IOKit assertion, logind or portal inhibitor, `SetThreadExecutionState`) could not be taken | Keep the computer awake | — |

The iPad has no metered-network signal here; it does not record, so nothing is
checked. While a capture runs on Android, its foreground service is sticky: if
Android kills the process, it restarts the service, which resumes the capture
from what was written. Before Android 12 a 120-second exact watchdog alarm
also restarts it. Patina does not ask for exact alarms, and from Android 12 an
inexact alarm may neither come on time nor start a foreground service from
the background, so none is armed there. Nothing restarts a capture after a
reboot or a force stop.

The recorder tells the host whether a capture is running or due
(`Ports::recording_schedule`). A desktop capture can continue after the
window closes while the host process and computer stay running. Patina
registers no wakes, login items or alarms for scheduled recordings. See
[architecture](architecture.md) and
[platform installation details](../README.md) for the supported distributions.

Validation combines portable profile/decision tests, native startup/restart
tests, headless control scenarios and native host smoke checks. Real-device
and emulator evidence must remain distinguishable.

The panel timeout rows are Advanced settings on TV and hidden elsewhere.
On phones, tablets and desktops their stored values are ignored, both at
startup and after restoring a backup; fixed defaults apply, extended by the
system recommendation and suspended while assistive technology runs. TV
keeps the stored values. No stored values are deleted or migrated.
