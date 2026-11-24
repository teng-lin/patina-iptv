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
| Connected input: a hardware keyboard, a D-pad | Android: `Configuration.keyboard` and `navigation`, unless hidden. Desktops: a keyboard. Tizen: its remote's D-pad. iPad: none yet (`GCKeyboard` is not read). | Keyboard and remote features |

Android reports the form factor and the input when the app starts and on every
configuration change; before that report an Android build counts as a tablet
with neither input. The settings offer check (`settings::offer`), the control
strip and multiview's decoder count read the signals as they are now, so a
change takes effect without a restart. A control that leaves the strip while
focused passes the focus to its nearest neighbor. The host profiles above still
follow the platform alone; Record now and the Recordings library follow the
form factor too.

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
| Android phone and tablet, Android 15 and later | The data-sync service's six hours a day | Android stops background recording after six hours | `onTimeout` ends the capture as partial, notifies, and stops the service |
| Desktop | The idle-sleep inhibitor (IOKit assertion, logind or portal inhibitor, `SetThreadExecutionState`) could not be taken | Keep the computer awake | — |

The iPad has no metered-network signal here; it does not record, so nothing is
checked. While a capture runs on Android, the 120-second watchdog alarm restarts
its foreground service if Android killed it; with no capture running no alarm
is armed. Patina does not ask for exact alarms: the watchdog is exact only
before Android 12, and Android does not rearm it after a reboot.

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

Multiview checks the main player, available decoder surfaces and extra player
windows at both presentation and action time. The provider's connection budget
and parental authorization remain separate checks at native admission; a
feature being available grants no permission to play content.

Patina does not schedule recordings, so no host registers a wake, a login
item or an alarm for one. The recorder tells the host only whether a capture is
running or due (`Ports::recording_schedule`): Android keeps its foreground
service and watchdog for it, and a desktop holds the computer from idle sleep.
Desktop capture can outlive the window while the host process and computer
remain running. iPad capture requires the app to remain open. Android uses a
foreground service and is subject to force-stop and OS execution limits. See
[architecture](architecture.md) and
[platform installation details](../README.md) for the supported distributions.

Validation combines portable profile/decision tests, native startup/restart
tests, headless control scenarios and native host smoke checks. Real-device
and emulator evidence must remain distinguishable.
