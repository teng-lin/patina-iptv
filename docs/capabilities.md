# Feature availability

The portable [capability model](../crates/iptv-core/src/capabilities.rs) separates
host facilities, live player reports and recording wake registration. The
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

These are configured limits and facilities, not certification of a device's
decoder performance. The profile tests exercise each row without needing that
OS. Android TV and tablet share the same surface budget; the native PiP report
also distinguishes a TV for the existing automatic-entry preference.

PiP starts unknown until the player reports support; hosts without a native
implementation acknowledge it as unavailable during setup. Its button and action
handler use the same current decision. A denied or changed report takes effect
without restarting the application. Desktop PiP presents the original player;
it never reserves another provider stream.

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

Awake-only desktop capture can outlive the window while the host process and
computer remain running. iPad capture requires the app to remain open. Android
uses a foreground service and is subject to force-stop and OS execution limits.
Windows wake tasks remain experimental; a successful registration is not
physical-machine wake validation. An unknown report does not promise an
unattended start. See [architecture](architecture.md) and
[platform installation details](../README.md) for the supported distributions.

Validation combines portable profile/decision tests, a real runtime test of
permission denial/grant/revocation without opening the catalogue, headless
control scenarios and native host smoke checks. Real-device and emulator
evidence must remain distinguishable.
