# Platform feature and operations audit

This inventory covers Patina’s viewer-facing features, the operations that make
them usable, and the settings and commands exposed by the shared app. Each row
has an ID for reviewing it independently. The proposed treatments are pending
decisions and use six labels:

| Label | Meaning |
| --- | --- |
| **Keep** | Offer it as it is, or with the presentation the row describes. |
| **Move** | Keep the operation, in its task context, under More or in advanced settings. |
| **Remove** | A product decision that this host does not offer it. Delete any entry points, and the code once nothing needs it. Keep persisted data readable and keep access to the viewer’s existing media and settings. |
| **Planned** | Wanted on this host but not built or not yet measured. No entry point, and no disabled placeholder, until it works. |
| **Hide** | Offered, but shown only while a runtime condition holds: a connected keyboard or D-pad, the window size, a handler or file picker, a permission, a free provider connection or content to show. Recheck it at execution. Show a disabled control with its reason only when the viewer can fix the condition. |
| **Add** | Viewers on the host expect an operation or setting that Patina does not offer and nobody has scheduled. |

The [removal and hiding decisions](#removal-and-hiding-decisions) section states
the principles and summarizes what each device loses. Three sections collect
what is absent: [missing features and operations](#missing-features-and-operations),
[missing settings](#missing-settings) and [missing workflows](#missing-workflows).
The [key workflows](#key-workflows) section describes the end-to-end journeys
the feature rows serve, and the [review record](#review-record) lists how five
independent reviews changed the proposal.

The source baseline is `469f3683` on `origin/master` (0.1.8). The local checkout
at `7580f8a4` has the same feature and settings code. The audit includes Android
handsets, Android tablets, iPad, Linux, macOS, Windows, Android TV and the two
Samsung Tizen hosts. The iOS package targets iPad. Android handsets have a
compact path that currently requests landscape on small full-screen devices.
Window width, text scale and active input can change independently of the host.
[Layout and input contract](interaction.md), [Android handset orientation](../hosts/android/app/src/main/kotlin/com/hantekllc/iptvrs/host/TvActivity.kt#L159), [iPad package](../crates/iptv-app/nsh.json#L18)

## Host profiles today

| Host | Playback and input | Recording and data operations |
| --- | --- | --- |
| Android phone | Touch-first. Shared Android host advertises two player surfaces. Native PiP depends on the device report. | Background capture and alarm registration use the shared Android host. The system document picker handles file operations. |
| Android tablet | Touch-first with keyboard and pointer support. Two reported slots in the host profile. Native PiP depends on the device report. | Same recording host and file operations as the phone. |
| iPad | Touch, keyboard and pointer. The profile allows up to four surfaces. Patina has no host PiP implementation. Visible split-screen playback can continue. | Foreground-only recording. Backup and restore appear as unfinished settings. |
| Linux and macOS | Pointer and keyboard, up to four surfaces, compact PiP and separate player windows. | The recorder can outlive the window while the computer stays awake. Start at login and portable backup and restore are available. |
| Windows preview | Pointer and keyboard, up to four configured surfaces, compact PiP. Separate player windows are Linux and macOS features. | Start at login and backup and restore are available. Wake-task registration is experimental and depends on the machine. |
| Android TV | D-pad and key phases, two configured surfaces. The shared Android player reports PiP using the device feature flag. | Shared Android recording host. File import and backup depend on a usable system picker. |
| Samsung Tizen | The 5.5 JavaScript bridge offers live TV, guide, replay where supported, search, favorites and history. The experimental NSH host exposes more of the shared shell with one decoder. | Both hosts lack a recording and portable-backup path. Source and player support differ from the native hosts. |

[Host capabilities](../crates/iptv-core/src/capabilities.rs#L19), [iPad lifecycle](interaction.md#interruptions-and-the-background), [desktop host differences](desktop.md#displays-and-hosts), [Android PiP report](../hosts/android/app/src/main/kotlin/com/hantekllc/iptvrs/host/PipHost.kt#L40), [Tizen bridge](../hosts/tizen/README.md), [Tizen NSH host](../hosts/tizen-nsh/README.md)

## Removal and hiding decisions

### Principles

1. Offer an operation on a device only when the device can complete it
   reliably and its viewers habitually do it there.
2. A failure the viewer discovers later, such as a recording that never
   started, is worse than not offering the operation.
3. Prefer a fixed or automatic behavior to a setting when one value suits
   nearly everyone or the platform already decides. Removing a setting must
   not remove an accessibility accommodation the platform does not provide.
4. Never ship a placeholder. Unbuilt or unmeasured work is Planned, with no
   entry point.
5. Move setup, organization and repair operations rather than removing them.
   Patina has no sync, so a phone may be a viewer’s only device.
6. Removing an operation keeps the viewer’s existing data reachable: recorded
   media, cancellation of pending work and persisted settings. Each removed
   setting states whether its stored value is ignored or migrated.

### Prerequisite: device, window and input signals

The capability model knows the operating system but not the device.
`Platform::Android` covers phones, tablets and TVs, and `REMOTE_KEYS` is a
build-time constant that is true on every Android build, so a phone shows the
TV remote settings and is offered multiview. NSH’s idiom cannot replace it: on
Android it follows the current window’s smallest width, so a tablet in split
screen or a small DeX window reports Phone. The settings offer check takes
only the key, `HOST` is a constant, and the control strip’s focus indexes a
fixed array, so every gate below also has to become a runtime input. Three
signals are needed:

| Signal | Source | Gates |
| --- | --- | --- |
| Device form factor | Android TV UI mode or the leanback feature, the physical display’s smallest width, the iPad idiom, the desktop build | Capabilities: Record now, the library, system PiP, audio delay, exit behavior |
| Window size class | The existing layout class (compact, medium, expanded) | Layout: multiview tiles, browse modes, video transparency |
| Connected input | A hardware keyboard or D-pad: Android `Configuration.keyboard` and `navigation`, iPadOS `GCKeyboard` | Keyboard and Remote settings, preview and zap timing |

When a signal changes during a session, Stop recording stays reachable and
multiview collapses to the tile being heard.
[Platform](../crates/iptv-core/src/capabilities.rs#L5), [remote keys constant](../crates/iptv-app/src/host.rs#L62), [phone detection](../crates/iptv-app/src/scale.rs#L45), [layout class](../crates/iptv-app/src/layout.rs#L5), [multiview admission](../crates/iptv-presentation/src/capabilities.rs#L51), [settings offer](../crates/iptv-app/src/settings.rs#L170), [constant host](../crates/iptv-app/src/capabilities.rs#L6)

### Scheduled recording is removed on every host

Scheduled recording works on some devices in some configurations and fails
silently on the rest, and Patina cannot promise which at the moment a viewer
saves a job:

| Host | What an unattended start depends on |
| --- | --- |
| Android phone and tablet | Exact alarms, pre-granted through Android 13 and denied by default to new installs on Android 14 and later. `USE_EXACT_ALARM` is granted automatically but Play restricts it to alarm and calendar apps. Without exact alarms Patina’s inexact alarm can arrive up to an hour late on Android 12 and later, and its receiver then starts a foreground service from the background, which those versions refuse unless an exemption applies, such as an exact alarm or the viewer setting battery use to Unrestricted. A refused start retries every 60 seconds. Doze and OEM battery managers add delays, and the `dataSync` service type allows six hours of background time a day for apps targeting Android 15. |
| Android TV | The same permission rules, although many boxes run Android 9 to 11, where exact alarms and background starts are allowed. Low Power Standby (Android TV 13 and later) and the Android TV 14 energy modes cut network access and ignore wakelocks in standby, even for foreground services, unless the app is exempt. Patina can detect this but cannot change it. Comparable TV IPTV apps do offer scheduling. |
| iPad | Background execution, which Patina does not have. |
| Laptops | The lid action, which sleeps the machine unless it is configured otherwise or driving an external display. Scheduling a wake needs root on macOS and privileges on Linux, and Windows wake timers depend on a power setting often off on battery. |
| Always-on desktop | An idle-sleep assertion, which needs no privileges on any desktop but which Patina does not hold (F18), start at login and surviving overnight OS update restarts. |

The owner has decided to remove scheduling, recurrence, padding, wake
registration, alarm access, the desktop login recorder and the Windows wake
task on every host (R02–R04). This is a scope decision: reliability varies by
device and setting, a failure is discovered after the program has ended, and
the wake, alarm and login code is a large surface for a small team. Record now
stays where the viewer is present when it starts. Catch-up remains the path
for watching a program later where the provider keeps an archive; where it
does not, that journey is lost (W21). Scheduling can return only as an
explicit recorder mode on a mains-powered desktop or Android TV box that
passes runtime checks (exact alarms, no Low Power Standby restriction,
unrestricted battery use), holds a sleep assertion, shows that it is running
and reports every job’s result.
[Inexact fallback](../hosts/android/app/src/main/kotlin/com/hantekllc/iptvrs/host/RecordingHost.kt#L133), [background service start](../hosts/android/app/src/main/kotlin/com/hantekllc/iptvrs/host/RecordingHost.kt#L149), [wake report](../crates/iptv-app/src/host/native/recording_host/android.rs#L60), [minimum SDK](../hosts/android/app/build.gradle.kts#L37), [Android alarms](https://developer.android.com/develop/background-work/services/alarms), [background start exemptions](https://developer.android.com/develop/background-work/services/fgs/restrictions-bg-start), [service timeouts](https://developer.android.com/develop/background-work/services/fgs/timeout), [Low Power Standby](https://developer.android.com/reference/android/os/PowerManager), [Android TV 14 energy modes](https://9to5google.com/?p=619800), [macOS power events need root](https://opensource.apple.com/source/IOKitUser/IOKitUser-294/pwr_mgt.subproj/IOPMLib.h.auto.html)

### Summary by device

| Area | Phone | Android tablet | iPad | Desktop | Android TV | Tizen bridge | Tizen NSH |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Scheduled and repeating recording (R02–R04) | Remove | Remove | Remove | Remove | Remove | Remove | Remove |
| Record now (R01) | Move under More, with checks | Move under More, with checks | Remove | Keep once F18 holds the machine awake | Keep, with standby and storage checks | Remove | Remove; its rows are offered and fail today |
| Recordings library (R05) | Hide when empty | Hide when empty | Hide when empty | Keep | Keep | Remove | Remove |
| System picture in picture (F05) | Keep | Keep | Planned | Keep, manual | Remove on every build | Remove | Remove |
| Multiview (F06) | Move under More: two tiles, full-screen landscape | Keep, two tiles, expanded window | Planned until measured | Keep; Windows has no player windows | Keep, two tiles | Remove | Remove |
| External player (F13) | Keep in group options; Hide without a handler | Same | Remove | Linux: Keep, Hide without a handler. macOS, Windows: Remove | Keep in group options; Hide without a handler | Remove | Remove |
| Backup and restore (F14) | Keep | Keep | Planned | Keep | Hide import and export without a picker; pending restore always reachable | Remove | Remove |
| Audio delay (F03) | Move under More | Move under More | Move under More | Move under More | Move under More | As reported | As reported |
| Keyboard and Remote settings (S08–S09) | Hide unless a keyboard or D-pad is connected | Same | Same | Keyboard settings | Remote settings | Bridge setup | Remote settings |
| Panel timeouts (S06) | Keep under Accessibility | Same | Same | Same | Move to advanced | Bridge setup | Move to advanced |
| Preview and zap timing (S05) | Hide unless a keyboard or D-pad is connected; Remove the rest | Same | Same | Move to advanced or Remove per row | Move to advanced | Bridge setup | Move to advanced |
| Exit confirmation (S02) | Remove | Remove | Remove | Remove | Remove | Fixed Yes/No popup | Fixed Yes/No popup |

Everywhere, remove the disabled placeholders (F16), hide Movies and Shows when
every enabled playlist is M3U (F09), and remove the settings that S01, S02 and
S10–S14 replace with fixed or automatic behavior. An Android phone offers
about 64 settings rows today and would offer about 29 after this pass, before
the additions in S16–S26.

### Kept on purpose

- Playlist setup, guide repair, bulk organization and backup on phones move
  deeper rather than disappear (principle 5). Parental controls stay at their
  usual depth on phones, which parents hand to children.
- The sleep timer stays on every host. Phones and tablets are used in bed, and
  set-top boxes have no system sleep timer.
- Panel timeouts stay under Accessibility until the panels follow the
  platform’s accessibility timeout (S06).
- iPad picture in picture, multiview and backup are Planned, not removed. iPad
  picture in picture needs an `AVPictureInPictureController` fed from mpv
  through an `AVSampleBufferDisplayLayer`, the background audio mode and a
  stream that keeps running in the background, the same pieces as F19 and
  W15. Plan them as one iPad background-playback project.

## Features and operations

Phone means Android phone. Tablet includes Android tablet and iPad, and TV
includes Android TV and Tizen where a cell calls them out. The host profiles
above describe current support. The columns below propose how to present it.

| ID | Feature and operations | Phone | Tablet | Desktop | TV | Evidence |
| --- | --- | --- | --- | --- | --- | --- |
| F01 | Live TV. Browse channels and groups, tune, open or close the guide and channel list, return to the player, zap and go to the previous channel. | Keep. Lead with a compact list and a separate Guide action. Allow portrait for setup, settings, search and text entry, and turn to landscape when video goes full screen. Review a portrait guide separately. | Keep the grid and video preview. Collapse the layout in narrow windows. | Keep grid, list, window resizing and wheel or keyboard navigation. | Keep the grid, preview and remote zapping as first-class paths. Use the bridge’s supported equivalent on Tizen. | [Interaction contract](interaction.md#choosing-and-inspecting) |
| F02 | Program navigation. Show now and next, inspect details, jump to now or another time, tune a current listing and open catch-up for an eligible past listing. | Keep Now, Next and Replay. Put day and hour jumps behind a date control when space is tight. | Keep the time grid, pan and jump controls. | Keep the grid, jump controls and shortcuts. | Keep Back to now, Guide and remote day/hour navigation. Keep title and time where Tizen has no synopsis. | [Guide layout](interaction.md#layout-classes), [TV playback](tv-controls.md), [Tizen limits](../hosts/tizen/README.md#known-limits) |
| F03 | Playback controls. Play, pause, stop, retry, seek and return to live. Choose audio, captions, video track, picture size, mute, volume, audio delay and sleep timer. Enter and leave full screen. | Keep essential controls and captions visible. Put track and picture choices and audio delay under More; audio delay also fixes sound that is out of sync in the source. Keep the sleep timer. | Same. | Keep keyboard and pointer equivalents, including window full screen. Move audio delay under More. Keep the sleep timer. | Keep D-pad access and the sleep timer. Give captions a predictable position. Move audio delay under More. Hide each control the current player report cannot serve. Tizen bridge gets its supported subset. | [Player controls](../crates/iptv-app/src/player_panels.rs#L134), [play bar](../crates/iptv-app/src/play_bar.rs), [audio delay availability](../crates/iptv-app/src/media.rs#L35), [Tizen bridge differences](../hosts/tizen/README.md#differences-from-the-android-tv-app) |
| F04 | Live startup and interruptions. Launch, resume last channel, recover a failed stream, handle audio or route interruptions, leave and return. | Keep recovery and explicit resume. Start new installs on the guide with resume off, to avoid surprise playback in public and mobile data use. | Keep resume as a choice, off for new installs. Preserve visible split-screen playback. | Keep the launch preference as the viewer’s choice. Distinguish minimize, close and quit. | Keep last-channel startup on by default, since turning the TV on to live playback is the established habit, and a predictable Back path. Tizen follows its host’s startup and exit contract. | [Input and background contract](interaction.md#interruptions-and-the-background), [desktop close behavior](desktop.md#closing), [resume default](../crates/iptv-settings/src/catalog/general.rs#L56) |
| F05 | Picture in picture. Enter, toggle automatic entry, pause or zap in the compact window, return and close. | Keep manual entry with a positive report. Enter automatically on Home during full-screen playback, the Android handset norm, and not from the guide preview. Honor an existing opt-out. | Same on Android. Planned on iPad: no control until it works, instead of today’s disabled “unavailable on this device”. | Keep manual entry, return and close on the same player. Desktop has no automatic entry. | Remove system PiP on every Android TV build, not only Play builds. Today automatic entry is live on TV: the setting is offered on every Android host and the TV host enters PiP when the viewer leaves. Keep the in-app preview. Remove on Tizen. | [Patina PiP](../crates/iptv-app/src/pip.rs#L73), [TV entry on leaving](../hosts/android/app/src/main/kotlin/com/hantekllc/iptvrs/host/PipHost.kt#L142), [automatic PiP offer](../crates/iptv-app/src/settings.rs#L200), [unsupported reason](../crates/iptv-app/src/capabilities.rs#L24), [Android TV permitted PiP uses](https://developer.android.com/training/tv/get-started/multitasking#usage-types) |
| F06 | Multiview and player windows. Add, replace and remove tiles, enlarge or reduce, select which tile is heard, open or close a separate window. | Move under More: two tiles, in full-screen landscape only, where a tile is about the width of a portrait phone’s video. Hide it on a metered network and at the account’s connection limit. Remove sound follows tile. Comparable live-TV apps offer phone multiview. | Android tablet: Keep two tiles in an expanded window. iPad: Planned until measured, since iPad draws every mpv frame on the main thread, and gated by window size class so Slide Over never gets tiles. | Keep multiview and Linux/macOS player windows. Keep Windows at a validated performance budget. | Keep two-tile multiview on Android TV. Show it disabled with the reason at the account’s connection limit. Add a decoder-capacity probe; today the two slots are a constant. Remove on both Tizen hosts. | [Multiview admission](../crates/iptv-presentation/src/capabilities.rs#L51), [decoder slots](../crates/iptv-core/src/capabilities.rs#L19), [iPad render profile](../crates/iptv-player-mpv/src/mpv/handle.rs#L96), [desktop players](desktop.md#several-players), [sound behavior](interaction.md#which-player-is-heard) |
| F07 | Find content. Search channels and programs, open results, use or clear recent searches and find replayable listings. | Keep visible search. Put time and replay filters in results. | Keep search and filters. | Keep search, keyboard shortcut and result filters. | Keep remote and system keyboard paths. Preserve focus and result position when returning from playback. | [Search shell](../crates/iptv-app/src/search_view.rs), [Tizen input](../hosts/tizen/README.md#input-and-view-protocol) |
| F08 | Favorites, groups and history. Add or remove favorite, reorder, create, rename, hide, block or restore groups and channels. Play from history, clear viewing history and go to the previous channel. | Keep favorite and recent channels. Move bulk edits into Manage. | Keep visible group actions and bulk edits. | Keep right-click, visible menus, ordering and multi-item organization. | Keep favorites, history and common mutations. Put bulk editing one level deeper with D-pad confirmation. Tizen bridge has favorites and favorites order. | [Secondary actions](interaction.md#secondary-actions), [group menus](../crates/iptv-app/src/menu_view.rs#L757) |
| F09 | Movies and series. Import supported catalogs, browse, filter, sort, open details, select season or episode, play, resume, play next and return to details. | Keep where the host can import a catalog. Lead with continuation and one pane at a time. | Keep the full catalog. Use compact details in split screen. | Keep full discovery and sorting. | Keep Movies, Shows and useful shelves on Android TV. Remove from the live-only Tizen bridge. Evaluate each catalog and playback operation on experimental Tizen separately. | [VOD and parental controls](../README.md#movies-series-and-parental-controls), [VOD empty states](interaction.md#empty-and-unavailable-states), [Tizen bridge](../hosts/tizen/README.md) |
| F10 | Playlists. Add by supported protocol or file, edit credentials and headers, retry failed imports, enable or disable, rename, reorder, choose one or combine, refresh and delete. | Keep all supported source types and system file picker. Lead with paste or file import. | Keep setup and account management. | Keep full management and file operations. | Keep URL and account setup, refresh and deletion. Hide file import when no remote-usable picker resolves. Tizen bridge uses M3U or Xtream setup. | [Source features](../README.md#features), [playlist actions](../crates/iptv-app/src/playlists_view.rs#L564), [Tizen setup](../hosts/tizen/README.md#using-it) |
| F11 | Guide sources and repair. Add and edit XMLTV feeds, enable, order, offset and match, refresh and clear cached listings. Inspect, preview, apply, verify and undo a repair where supported. | Keep refresh and status in context. Move repair and feed ordering into the playlist’s guide tools; do not remove them, since the phone may be the only device. | Keep guide tools and repair where supported. | Keep the full guide repair workflow and clear scoped status. | Keep refresh, feed setup and status. Move repair into advanced source settings. Tizen bridge keeps its own guide import and refresh path. | [Guide settings commands](../crates/iptv-app/src/settings_view.rs#L325), [repair lifecycle](runtime-api-v1.md#repair-lifecycle-and-completion) |
| F12 | Parental controls. Enable protection, block and unblock, unlock, change PIN, use recovery, relock and protect settings. | Keep at its usual depth on native hosts; parents hand phones to children. | Keep, prominently: tablets are often family devices. Include accessible entry and recovery. | Keep the same protections for pointer and keyboard. | Keep prominently on shared household TVs. Remove from the Tizen bridge unless an equivalent end-to-end protection exists, and say so in its listing. | [Parental controls](../README.md#movies-series-and-parental-controls), [admission](interaction.md#admission-and-capability) |
| F13 | External player. Set a group preference, launch or decline a handoff, release the connection and return to Patina. | Keep in group options. Hide it when no handler resolves, recheck before each handoff and play internally if the handler is gone, with a way to reset the group preference. Remove the disabled External player row from the program menu on every host. | Same on Android. Remove on iPad. | Keep on Linux, hidden when no handler is usable. macOS and Windows: Remove, which is already the state. | Keep in plain group options on Android TV, where viewers hand streams to another player for passthrough and frame-rate switching. Verify that Patina’s loopback relay keeps serving the handoff while Patina is in the background, since Android 14 freezes cached processes. Remove on Tizen. | [Host support](../crates/iptv-app/src/host/native/external_player.rs#L6), [group menu](../crates/iptv-app/src/menu_view.rs#L757), [handoff](../crates/iptv-app/src/external_player.rs#L229), [program actions](../crates/iptv-presentation/src/menu.rs#L186) |
| F14 | Portable backup and restore. Choose destination or source, review contents, schedule or cancel restore, restart and recover. | Keep on Android in Data and privacy, for moving to a new phone. | Keep on Android. Planned on iPad: remove the unfinished rows until it works. | Keep file import, export, review and recovery. | Keep on Android TV. Hide import and export separately when no document picker resolves; keep a staged restore’s review and cancellation reachable, since they need no picker. Remove on Tizen. | [Settings availability](../crates/iptv-app/src/settings.rs#L170), [restore entry](../crates/iptv-app/src/backup.rs#L73), [Android file picker](../hosts/android/app/src/main/kotlin/com/hantekllc/iptvrs/host/BackupComponent.kt#L201), [export security](security.md#export-recordings-and-local-authority) |
| F15 | Input and accessibility. Touch, D-pad, pointer, keyboard, system text size and screen reader, visible focus, shortcuts and remapping. | Keep touch, accessibility and hardware-keyboard support. Show Keyboard settings while a keyboard is connected, so single-key shortcuts can be remapped (WCAG 2.1.4), and Remote settings while a D-pad or controller is connected. | Same. | Keep shortcuts and focus. Present customization as Keyboard settings. | Keep D-pad navigation, visible focus, key customization where available and a Menu alternative to holds. | [Input modes](interaction.md#input-modes), [shortcut keys](../crates/iptv-app/src/input/keys.rs#L17), [accessibility journeys](accessibility.md), [remote editor](../crates/iptv-app/src/remote_editor.rs) |
| F16 | Placeholders and unsupported functions. The rail shows a disabled My List. The program menu shows a disabled Save to my list on every program, Remind me on upcoming programs and External player on live or replayable ones. The control strip draws all 15 controls, with Channel options and Settings permanently disabled and Record, Recordings, PiP and Multiview disabled on hosts that can never offer them. On the Tizen NSH host the Record and Schedule rows and the details Record button are enabled and fail. | Remove the strip’s Channel options and Settings and the program menu’s External player row. My List and Save to my list are Planned with a VOD watchlist; Remind me is Planned and belongs first on phones as a notification. Omit every control this audit removes for the host. | Same. On iPad, App Store Guideline 2.1 also rejects placeholder content. | Same. | Same. Fix the Tizen NSH recording rows now. Saved key mappings already cannot call Record or multiview; gate the default Record key instead. | [Rail items](../crates/iptv-presentation/src/browse.rs#L50), [program menu](../crates/iptv-presentation/src/menu.rs#L181), [implemented actions](../crates/iptv-app/src/menu_view.rs#L29), [recording rows on the web host](../crates/iptv-app/src/menu_view.rs#L149), [control strip](../crates/iptv-app/src/player_panels.rs#L135), [remappable actions](../crates/iptv-nav/src/remap.rs#L130), [default Record key](../crates/iptv-nav/src/keymap.rs#L239) |
| F17 | Library startup and failure recovery. Report unavailable or locked storage, recover a staged restore, retry startup and reopen. | Keep error and recovery actions within the launch flow. | Keep launch recovery and a readable error at split-screen sizes. | Keep reopen and restore when available. Explain how to return to an existing instance when it owns the catalog. | Keep remote-reachable recovery on Android TV. Treat browser storage and Tizen bridge startup as separate recovery paths. | [App startup](../crates/iptv-app/src/lib.rs#L772), [Tizen bridge startup](../hosts/tizen/README.md#using-it), [browser storage](security.md#browser-build-of-the-app) |

The rail adds Movies and Shows even when no enabled playlist can supply them,
a deliberate choice in `62f4d89b` to keep them discoverable. M3U imports never
produce a catalog, so this pass reverses that choice in one case: once at
least one playlist exists, hide both entries when every enabled playlist is
M3U. Keep them before the first playlist and for Xtream and Stalker sources,
including those with import off, where the empty state links to the
playlist’s settings. Apply the same rule to the VOD browser’s own navigation.
[Rail items](../crates/iptv-presentation/src/browse.rs#L86), [VOD navigation](../crates/iptv-app/src/vod_view/browser.rs#L76), [M3U catalogs](../README.md#movies-series-and-parental-controls)

### Current entry points and limits

| IDs | Where the operations start today | What changes the result |
| --- | --- | --- |
| F01–F02, F04 | Guide, channel list, video preview, title and program menu, and navigation keys. Resume on launch is a shared preference. | The active input and layout change the navigation path. Catch-up requires an eligible program and provider archive. Tizen bridge search results tune the channel live, even where its guide can open replay. [Choosing and inspecting](interaction.md#choosing-and-inspecting), [TV keys](tv-controls.md), [Tizen differences](../hosts/tizen/README.md#differences-from-the-android-tv-app) |
| F03, F05–F06 | Play bar, control strip, the info card’s action cards, More controls, multiview picker and desktop window actions. Android also handles Home for automatic PiP. | Seek depends on the media and session. Extra players consume host slots and may consume more provider connections. Automatic PiP is offered on every Android host, TV included. AVPlay in the experimental Tizen host has limited track, volume and format reporting. Without local timeshift, what Pause does on a live stream is not defined: a bounded buffer with Back to live, or Stop and Mute in its place. [Play bar](../crates/iptv-app/src/play_bar.rs#L362), [action cards](../crates/iptv-presentation/src/panels.rs#L50), [PiP](../crates/iptv-app/src/pip.rs), [desktop players](desktop.md#several-players), [Tizen NSH limits](../hosts/tizen-nsh/README.md#what-does-not-work-yet) |
| F07–F09 | Search rail and keys, guide and channel menus, Favorites or Recently watched lists, Movies and Shows rail entries, shelves and detail screens. | Search, ordering and visibility use the enabled catalog. Movies and Shows appear before VOD is imported. Shelves depend on VOD availability. Protected playback requires a content unlock. Tizen bridge has its own Search and Favorite keys and a live-only catalog. [Browse and remapping](tv-controls.md), [rail rules](../crates/iptv-presentation/src/browse.rs#L83), [VOD behavior](../README.md#movies-series-and-parental-controls), [Tizen controls](../hosts/tizen/README.md#using-it) |
| F10–F11 | First-run wizard, Playlists settings and playlist actions, EPG settings and the playlist or channel’s guide tools. | URL, login, picker and import support differ by host. The Tizen bridge keeps its own M3U/Xtream setup. The experimental browser shell can import guide feeds but AVPlay refuses streams requiring custom request headers. [Source management](../crates/iptv-app/src/playlists_view.rs), [guide tools](../crates/iptv-app/src/epg_sources_view.rs), [Tizen NSH](../hosts/tizen-nsh/README.md#how-it-runs) |
| F12–F14 | Channel, group and VOD block actions, PIN prompts and settings. External player lives in group options. Backup and Restore live in General and require authorization when protected. | A content unlock has narrower scope than administrative access. External handoff depends on a supported host and external handler. Backup and restore depend on workers and usable file dialogs, with an unfinished row on iPad. [Parental controls](../README.md#movies-series-and-parental-controls), [group options](../crates/iptv-app/src/menu_view.rs#L757), [backup and restore](../crates/iptv-app/src/backup.rs#L37) |
| F15–F17 | Accessibility uses native semantics and text scale. Remote mappings live in Settings. The shared rail includes a disabled Watchlist. Storage and restore failures can appear before Settings is reachable. | Raw key phases come from the Android and browser TV hosts, including on Android handhelds with keyed input. Desktop has keyboard mappings. Assistive-technology activation and speech depend on the host integration and device. [Input profiles](../crates/iptv-app/src/input.rs#L69), [rail items](../crates/iptv-presentation/src/browse.rs#L83), [accessibility journeys](accessibility.md), [launch recovery](../crates/iptv-app/src/lib.rs#L772) |

## Missing features and operations

The operations below are absent on every host or on most of them, so these rows
propose additions rather than a presentation. Viewers on the named devices
expect them from the platform itself or from comparable video apps. The columns
follow the feature table. None has an entry point today, apart from the Android
and Tizen screen hold, the Back to live key and the hidden settings named in the
rows. The rows come from code search at `7580f8a4`; F18, F26 and F28 need
device confirmation.

| ID | Feature and operations | Phone | Tablet | Desktop | TV | Evidence |
| --- | --- | --- | --- | --- | --- | --- |
| F18 | Screen and system sleep. Keep the screen on while video plays and keep the computer awake while a Record now capture runs. | Keep. The Android player holds the screen on while it plays, and a capture holds a partial wake lock. | Add on iPad by disabling the idle timer during playback. Android keeps the screen on. | Add, before Record now ships on desktop (R01). mpv renders through `vo=libmpv`, which never inhibits the screensaver, and no OS sleep API is called. Inhibit display sleep during playback and hold an idle-sleep assertion during capture, which needs no privileges on Linux, macOS or Windows. | Keep on Android TV and Tizen, where playback holds the screen on. | [libmpv output](../crates/iptv-player-mpv/src/mpv/handle.rs#L77), [Android screen flag](../hosts/android/app/src/main/kotlin/com/hantekllc/iptvrs/host/PlayerComponent.kt#L298), [capture wake lock](../hosts/android/app/src/main/kotlin/com/hantekllc/iptvrs/host/RecordingHost.kt#L220), [Tizen screensaver](../hosts/tizen/app/main.js#L212) |
| F19 | Media session and system controls. Headset and keyboard media keys while unfocused, lock screen, Control Center, now-playing widgets and voice transport commands. | Add an Android media session. | Add a media session on Android. On iPad, plan Now Playing with picture in picture and background audio as one project. | Add MPRIS on Linux, Now Playing on macOS and system media transport controls on Windows. | Add a media session on Android TV for Assistant transport commands. Tizen keeps its registered keys. | No host integration today. |
| F20 | Casting. Send playback to a Google Cast or AirPlay receiver. | Remove as a non-goal. Provider tokens tied to an IP address or User-Agent, and custom request headers, often fail on cast receivers. | Same. AirPlay video would need AVPlayer rather than mpv. | Not applicable. | Not applicable. | No sender integration today. |
| F21 | Open with and share. Open a playlist file or link from another app, or drop it onto the window. | Add VIEW for M3U files and links and SEND for shared text, routed to the add-playlist flow with confirmation. | Same on Android. Declare the document types on iPad. | Add drag and drop onto the window and file associations for `.m3u` and `.patinabackup` in installed builds. | Not applicable. | [Launcher-only activity](../hosts/android/app/src/main/AndroidManifest.xml#L64) |
| F22 | Setup help on a TV. Enter a link, server or password without typing it with the D-pad. | Can act as the companion that sends the setup to a TV. | Same. | Not applicable. | Add a phone-assisted path, such as a QR code to a short-lived local form or an entry code, after a security review. Keep password fields revealable and editable rather than retyped. Backup and restore is no substitute where the TV lacks a usable file picker. | [Playlist setup](../crates/iptv-app/src/playlists_view.rs), [export security](security.md#export-recordings-and-local-authority) |
| F23 | Radio and audio-only channels. | Use the parsed radio flag to show the logo and program instead of black video. Consider background audio for radio only. | Same. | Same, without background audio. | Same. | [Radio flag](../crates/iptv-providers/src/model.rs#L216) |
| F24 | Provider account status. Expiry, connection allowance and active connections where the provider reports them. | Show in playlist details and in playback errors about connection limits. | Same. | Same. | Same, readable from across the room. | [Xtream expiry](../crates/iptv-providers/src/xtream/parse.rs#L51), [Stalker expiry](../crates/iptv-providers/src/stalker/parse.rs#L155) |
| F25 | On-demand progress. Remove a title from Continue watching, mark watched or unwatched and forget viewing time. | Add to the title’s actions. | Same. | Same, including the right-click menu. | Same, under the title’s Menu. | [VOD and parental controls](../README.md#movies-series-and-parental-controls), `general.reset_watch_time` is hidden |
| F26 | Replay navigation without keys. Restart the current program and go back to live. | Add Restart and Back to live to the play bar where the stream supports them. Today Back to live is reachable only from a key. | Same. | Same. Keep the N key. | Keep ⏭ and remote mappings, and add the play-bar controls. | [Play bar controls](../crates/iptv-presentation/src/play_bar.rs#L21), [Back to live dispatch](../crates/iptv-app/src/player_keys.rs#L127) |
| F27 | Diagnostics. Stream information, such as codec, resolution and dropped frames, and a log export for support. | Add stream information under More and a log export in About. | Same. | Same. | Same. | `player.show_media_info` and `general.send_logs` are hidden |
| F28 | Track choices across channel changes. Keep the viewer’s audio language and subtitle choice when zapping, returning to a channel or opening the next episode. | Add. Apply the S16 and S17 preferences to each new stream. Today every new stream starts with automatic audio and subtitles off. | Same. | Same. | Same. Captions that switch off on every zap fail the viewers who depend on them. | [Track reset per stream](../crates/iptv-player-mpv/src/mpv/worker.rs#L425), [choices forgotten](../crates/iptv-app/src/media.rs#L5) |
| F29 | Zapping by touch. Change channel from full-screen live video without a keyboard or remote. | Add a vertical swipe on full-screen live video, starting away from the screen edges: up for the next channel, down for the previous. The banner follows the finger, a release past the threshold commits, and tuning waits for the switch delay as Ch± does. Add next and previous channel buttons to the play bar and accessibility actions on the video, as WCAG 2.5.1 requires an alternative to a path gesture. One setting, Swipe to change channel, on by default. Not in replay, on-demand playback or multiview. | Same. On iPad, VoiceOver’s swipe up and down can zap through an adjustable element. | Not applicable: keys zap. | Not applicable: the remote zaps. | No swipe or channel button on the full-screen player today. [Zap path](../crates/iptv-app/src/browse_playback.rs#L190), #285 |

## Recording operations

Today Record on the play bar or remote can start or stop capture. Menu on a
current guide program can record without changing the player. On a future
program, Schedule recording saves the guide times immediately. The manual
scheduler opens an editor with Save, repeat and padding. The Recordings rail
entry opens the library and its edit, cancel, stop, play and delete actions.
The runtime handles schedule admission and wake acknowledgement separately.
Record now itself runs through the scheduler: it saves a one-off job that
starts at once.
[Current recording journeys](../README.md#recordings-on-tv-tablet-and-desktop), [schedule admission and wake report](../crates/iptv-runtime/src/recordings.rs#L595), [Record now saves a job](../crates/iptv-app/src/recordings_view.rs#L550), [recording actions](../crates/iptv-app/src/recordings_view.rs#L722)

The proposal keeps only capture that the viewer starts while present, for the
reasons in [scheduled recording is removed](#scheduled-recording-is-removed-on-every-host).

| ID | Operation | Proposed treatment |
| --- | --- | --- |
| R01 | Record now, stop capture and retain the partial file. | Keep on Android TV and desktop, from playback and the current program’s menu. Move under More on Android phones and tablets. Remove on iPad and both Tizen hosts. Before starting, check free space against the budget, a metered network on handhelds (S19) and, as today, a free provider connection. On Android TV, check Low Power Standby and, where it restricts Patina, say that recording stops when the TV is turned off, then close the partial file when it does. On desktop, ship it only once F18 holds the machine awake during capture, and warn until then. On macOS, closing the window keeps the capture running with the Dock icon and Quit asks first; on Windows and Linux, closing the window asks whether to stop recording and quit, so no recorder runs without a window. On Android, describe the six-hour background limit when it applies and notify the viewer when a background capture fails. Before Play distribution, check the service type, since Google lists TV recording under `mediaPlayback` rather than `dataSync`. |
| R02 | Schedule a guide program or enter a manual time. | Remove on every host: the Schedule recording and Schedule a recording… menu rows, the program details Record button and its Select and Record handling for upcoming and ended programs, the guide Record key on upcoming programs, the manual editor, repeat, padding and the guide’s scheduled badges. |
| R03 | Edit or cancel future jobs, repeat daily, weekly or by title, and cancel a series rule. | Remove creation and editing on every host. Jobs and rules carried over from earlier versions are cancelled by the migration below and shown read-only. |
| R04 | Register a wake, grant precise alarm access, start the desktop recorder at login, stop it and quit. | Remove: alarm access, the `SCHEDULE_EXACT_ALARM` permission, the boot, package-update and permission broadcasts and `RECEIVE_BOOT_COMPLETED`, the Windows wake task, desktop login registration and the background recorder launch. Keep what Record now needs: the job store, the schedule publication port, the Android foreground service start and a recovery path for a capture whose process was killed. Today that recovery is a 120-second watchdog alarm through the same receiver; keep it for active captures only or replace it with a sticky service restart and spool recovery. Reword the Android notification, which says “Scheduled recordings”, in every locale. |
| R05 | View current, missed and partial captures. Play, resume, play from beginning or delete retained media. | Keep on Android TV and desktop. On Android phones, Android tablets and iPad, Hide the library entry until a read confirms it holds media or carried-over jobs; iPad development builds can already hold captures. Remove on both Tizen hosts. Keep delete confirmation and separate Stop playback from Stop recording. Recording export does not exist; correct the store privacy text that claims it. |

The retirement has to work before any interface appears, because the Android
recording service and desktop login launches start the recorder first:

- **Store migration.** A database migration cancels scheduled jobs and
  disables series rules, so it applies on any upgrade path, including skipped
  releases, before any recorder starts. Existing migration files stay. Older
  backups that carry schedules restore without error and their schedules are
  dropped. Restore today discards recording rows although the media files
  survive outside the catalog; keep the metadata needed to reopen them.
- **Registrations outside the app.** On first launch, and on any launch with
  `--background-recordings`, remove the Linux autostart entry, the macOS
  LaunchAgent, the Windows task with its logon and time triggers and wake
  flag, and the `recording-login.enabled` marker. A launch with that flag then
  exits. Keep recognizing the flag for at least one release. On Android,
  cancel the pending recording alarm and clear the stored next-wake time.
- **Viewer notice.** Tell viewers once that scheduled recordings were removed,
  list the cancelled jobs and rules, and keep their media.
- **Stored settings.** Removed or hidden settings stay in `settings.json`,
  which keeps unknown keys, so each must be ignored or migrated, both at
  startup and when restoring a backup from another device. The settings rows
  below say which.
- **Documents and tests.** Update the README’s recording sections,
  `desktop.md`, `capabilities.md`, `interaction.md`, `tv-controls.md`,
  `accessibility.md`, `store-privacy.md`, the privacy policy, `security.md` and
  `runtime-recovery-validation.md`, and the scheduling, recorder-startup,
  desktop-close, capability and multiview tests.

[Login registrations](../crates/iptv-app/src/host/native/recording_host/startup.rs#L73), [Windows task](../crates/iptv-app/src/host/native/recording_host/windows.rs#L88), [background launch](../crates/iptv-app/src/desktop.rs#L25), [capture watchdog](../hosts/android/app/src/main/kotlin/com/hantekllc/iptvrs/host/RecordingHost.kt#L96), [restore discards jobs](../crates/iptv-runtime/src/backup.rs#L665), [Android manifest](../hosts/android/app/src/main/AndroidManifest.xml#L10), [service types](https://developer.android.com/develop/background-work/services/fgs/service-types)

## Settings and command inventory

The keys below cover the shared app’s wired settings, commands, two special
active keys and read-only key summary. Contextual operations such as per-group
sorting and guide feed offsets are listed with their owning features above.
Tizen bridge setup has its own smaller UI.
[Settings availability](../crates/iptv-app/src/settings.rs#L38), [settings pages](../crates/iptv-app/src/settings_view.rs#L50)

| ID | Settings and commands | Proposed home and availability |
| --- | --- | --- |
| S01 | `general.language`, `general.clock`, `appearance.font_size`, `about.privacy_policy` | Keep in General, Accessibility or About. On Android and iPad, follow the system’s 24-hour setting and remove the clock row; NSH already receives it. Keep the row on desktop and TV. Keep system-language and system-text defaults. |
| S02 | `general.resume_last_channel`, `general.confirm_exit`, `general.confirm_exit_window` | Keep resume as a launch preference with the F04 defaults, later folded into S20. Remove the two-press exit window and its timing on every host and ignore stored values. Android TV leaves on Back from the top level, following Google’s guidance against exit gating; Back already climbs from the guide through the groups and rail first. Android handhelds hand top-level Back to the system so the predictive back gesture works. Both Tizen hosts show a fixed Yes/No popup on Return at the first screen and leave the Exit key to the system, following Samsung’s guidance. That is new work on both: the bridge never leaves on Back and handles Exit itself, and the NSH host leaves on Back without a popup. [Default](../crates/iptv-settings/src/catalog/general.rs#L63), [Back handling](../crates/iptv-app/src/lib.rs#L1187), [bridge keys](../hosts/tizen/app/main.js#L797), [NSH host Back](../hosts/tizen-nsh/app/patina-host.js#L90), [Android TV navigation](https://developer.android.com/training/tv/get-started/navigation), [Samsung termination](https://developer.samsung.com/smarttv/develop/guides/fundamentals/terminating-applications.html) |
| S03 | `appearance.accent`, `appearance.background`, `appearance.ui_transparency` | Keep accent and background in Appearance. Hide video transparency in the compact window class, where the layout decides what draws over video. Protect captions and focus visibility independently of these choices. |
| S04 | `guide.browse_mode`, `guide.list_browse_mode`, `guide.visible_minutes`, `guide.show_logos`, `guide.show_channel_numbers` | Keep logos and channel numbers everywhere. Hide both browse modes in the compact window class, where the layout decides. Show the minute span only with a time grid. |
| S05 | `guide.autoplay`, `guide.switch_delay`, `guide.fullscreen_timeout`, `guide.stay_on_guide`, `guide.stay_on_list`, `guide.back_returns_to_now` | These tune focus-driven preview and zapping. Autoplay and switch delay: on handhelds, Hide unless a keyboard or D-pad is connected, and while hidden autoplay is off; Move to advanced on desktop and TV. Full-screen timeout, which turns a preview full screen after idle: Remove on handhelds and desktop, Move to advanced on TV. Stay settings: merge into one “After choosing a channel” choice on desktop and TV; on handhelds remove them. Activation is decided (2026-11-24): in the preview layout, on every device, the first choice tunes the preview and choosing the playing channel again goes full screen; the stay settings affect only the overlay layout and the channel list. `guide.back_returns_to_now`: Remove and keep its default on. [Activation](../crates/iptv-app/src/browse_playback.rs#L83) |
| S06 | `player.info_panel_timeout`, `player.controls_panel_timeout`, `player.info_on_switch` | Keep both panel timeouts under Accessibility on every host for now. Only the play bar waits for a screen reader, and Switch Access is not detected, so a fixed timeout would fail WCAG 2.2.1. Once the panels follow Android’s recommended accessibility timeout and hold while VoiceOver or Switch Control runs, remove them on handhelds and desktop and move them to advanced on TV. Hide `player.info_on_switch` on handhelds unless a keyboard or D-pad is connected: it applies only to zapping in full screen, which a touch-only handheld cannot do today, and touch zapping (F29) always shows the banner. Keep it on desktop and TV. [Play bar hold](../crates/iptv-app/src/play_bar.rs#L110), [strip auto-hide](../crates/iptv-app/src/player_panels.rs#L177), [Android accessibility timeout](https://developer.android.com/reference/android/view/accessibility/AccessibilityManager) |
| S07 | `player.pip_on_leave`, `player.sound_follows_tile` | Keep automatic PiP in Android handheld Playback, defaulting to full-screen playback only and honoring an existing opt-out (F05). Remove it on Android TV. Keep sound follows tile inside multiview, off by default, and Remove it on phones and wherever multiview is removed. |
| S08 | `catchup.seek_keys`, `catchup.forward_step`, `catchup.rewind_step`, `catchup.timebar_step` | Show seek keys with the Keyboard or Remote settings (S09). Move the forward and rewind steps to advanced Playback on every host; the touch play bar’s Rewind and Fast-forward buttons use them, and screen-reader users rely on those buttons. Keep the time-bar interval contextual. [Touch seek buttons](../crates/iptv-app/src/play_bar.rs#L688) |
| S09 | `remote.customize`, `remote.key_overrides`, `remote.long_press`, `remote.digit_timeout`, `remote.player_up_down` | On handhelds, Hide the page unless a hardware keyboard or D-pad is connected, not merely the last input used. Present it as Keyboard settings for a keyboard, which keeps shortcut remapping reachable (WCAG 2.1.4), and as Remote settings for a D-pad or game controller. On desktop, present Keyboard settings without remote-only keys. Keep Remote settings on TV. Remove the read-only overrides row everywhere in favor of the editor’s summary. Offer long press only for an input that reports key phases. |
| S10 | `playlists.manage`, `playlists.include_vod`, `playlists.max_connections`, `general.user_agent`, `vod.sort`, `vod.autoplay_next_episode` | Keep playlist management, the per-source VOD toggle and next-episode autoplay; Hide VOD-only rows when no enabled playlist can supply a catalog. Keep `general.user_agent` as a “Default User-Agent” in advanced source settings: it covers portals, guide feeds, on-demand and recording requests, which the per-playlist User-Agent for M3U and Xtream does not. The stream limit is already per playlist, and an explicit value overrides the provider’s report; move its editor to each playlist’s advanced settings, keep explicit values, show the provider-reported allowance beside it and migrate a stored global value into each playlist. Make the catalog’s Sort persist and migrate `vod.sort` into it before removing the row. [Global User-Agent use](../crates/iptv-runtime/src/store_service.rs#L509), [catalog sort](../crates/iptv-presentation/src/vod.rs#L193), [stream limit](../crates/iptv-runtime/src/recordings.rs#L1080) |
| S11 | `epg.add_source`, `epg.manage_sources`, `epg.update_now`, `epg.clear`, `epg.update_interval`, `epg.update_on_start`, `epg.update_on_playlist_change`, `epg.keep_past`, `epg.store_descriptions` | Group the commands under the relevant playlist’s Guide data with refresh status, Retry and the source to change, and confirm Clear with its source and scope. Merge the interval and start-up refresh into one “Update guide” choice. Remove the rest and ignore stored values: update after a playlist change always, store descriptions automatically per host, and derive past retention from the longest catch-up archive with a bounded floor for search’s past listings. |
| S12 | `search.favorites_first`, `search.include_past_without_catchup`, `search.stay_on_search`, `search.show_history`, `search.clear_history`, `history.clear` | Remove the first three and ignore stored values: rank favorites first always, offer past listings without catch-up as a filter in the results, and return to the results on Back. Keep search history, both Clear commands and an explanation of what clearing viewing history affects, in Data and privacy. |
| S13 | `parental.live_enabled`, `parental.vod_enabled`, `parental.require_pin_for`, `parental.pin_input`, `parental.unlock_window`, `parental.change_pin` | Keep protection and recovery together on supported hosts. Show VOD protection only where the host has a catalog. Number keys already work in both PIN modes; `parental.pin_input` only decides whether OK submits or presses the focused keypad key. Remove it once OK has one rule: submit when enough digits are entered, otherwise press the focused key. [PIN entry](../crates/iptv-app/src/parental_view.rs#L403) |
| S14 | `recordings.background_status`, `recordings.alarm_access`, `recordings.enable_startup`, `recordings.disable_startup`, `recordings.quit_host` | Remove all five with scheduled recording (R04). State Record now’s limits when the capture starts (R01). |
| S15 | `general.backup`, `general.restore` | Move to Data and privacy, reviewed with F14. Check export and import separately. Show pending restore, replace, cancel and restart together, reachable even where file import is hidden. Explain that export files contain credentials and that restore clears history. Planned on iPad and removed on Tizen. |

Keep wake launch, automatic installation of updates, telemetry, voice
preference, UDP relay, a forced decoder on TV, tunneling, audio passthrough and
automatic frame-rate controls out of the shared product settings until a host
has a complete implementation and an identified use case. S23, S24 and S25
propose narrower decisions for desktop decoding, boot launch on Android TV and
an update notice. A schema entry alone does not offer an operation. Keep
persisted keys compatible when removing or hiding rows.
[Hidden settings](../crates/iptv-app/src/settings.rs#L118), [settings catalog](../crates/iptv-settings/src/catalog/mod.rs)

Today the app builds the same Settings pages for all shared-shell hosts. The
offer check hides recording and backup settings on hosts without blocking
workers. Alarm access is Android-only. Login registration and recorder quit
are desktop-only. Backup and Restore are unfinished rows on iPad. Automatic
PiP is an Android setting, and Long press is available on raw-key-phase hosts.
Other wired settings and commands generally appear even when the current layout,
input or content gives them little practical effect. Per-playlist VOD import
is configured in playlist actions.
[Settings pages](../crates/iptv-app/src/settings_view.rs#L50), [current offer checks](../crates/iptv-app/src/settings.rs#L170), [playlist setting scope](../crates/iptv-app/src/settings.rs#L13)

## Missing settings

The settings below have no key in the schema, or have a hidden key that a
common workflow depends on. Where the platform already offers an equivalent,
such as system caption preferences, the setting follows it and appears in the
app only where the system has none.

| ID | Setting | Proposed home and availability | Evidence |
| --- | --- | --- | --- |
| S16 | Preferred audio language | Playback, on every host. Choose the first audio track in the preferred language, otherwise the stream’s default, and default to the interface language. Multi-language channels are common in IPTV catalogs, and F28 applies the choice to each new stream. | No key. [Each stream starts with automatic audio](../crates/iptv-player-mpv/src/mpv/worker.rs#L425) |
| S17 | Subtitles by default and preferred subtitle language | Playback and Accessibility, on every host: Off, On, or Only when the audio is not in my language. Follow the system caption switch where one exists, such as Android’s caption preferences and iPad’s Closed Captions and SDH. | No key. [Each stream starts with subtitles off](../crates/iptv-player-mpv/src/mpv/worker.rs#L426) |
| S18 | Caption appearance | Accessibility. Follow the system caption style where the OS has one and the player can apply it: Android, iPad, macOS and Windows. Offer size and background in the app on Linux, and as an override elsewhere. | No key. System caption preferences are not read. |
| S19 | Mobile data | Playback, on Android phones and tablets and cellular iPads. Ask before streaming on a metered network, with Always and Wi-Fi only choices, and defer automatic guide refreshes to unmetered networks. A live HD channel can use a gigabyte or more an hour. | No metered-network check in the hosts. |
| S20 | Launch destination | General. One choice in place of the resume toggle: Last channel, Guide on Favorites or Guide on all channels, with the F04 defaults per host. TV viewers who watch a few channels expect to start in Favorites. | [Resume toggle](../crates/iptv-settings/src/catalog/general.rs#L56), hidden [`channels.playlist_opens`](../crates/iptv-settings/src/catalog/general.rs#L247) |
| S21 | Pause and retain viewing history | Data and privacy. Pause history on a shared device, and keep it for a chosen number of days. Clearing stays a separate command (S12). | Hidden [`history.keep_days`](../crates/iptv-settings/src/catalog/library.rs#L64) and `history.add_after` |
| S22 | Recording storage | Recordings, on Android TV, Android handhelds and desktop. Desktop: choose the folder or drive and the space budget, today fixed at 32 GiB with 8 GiB per recording. Android: show free space and the budget, and offer removable storage where it is available to the app. Many TV sticks have little storage, so R01’s free-space check depends on it. | [Storage limits](../README.md#recordings-on-tv-tablet-and-desktop), hidden [`recordings.folder`](../crates/iptv-settings/src/catalog/library.rs#L128) |
| S23 | Desktop decoding and deinterlacing | Advanced Playback, on desktop only. Hardware decoding Automatic or Off, the usual fix for a faulty GPU driver, which today needs an environment variable. Deinterlacing Automatic, for interlaced broadcast channels; Patina sets no option, so the bundled mpv’s default needs checking. Decoder choices stay off TV as above. | [hwdec from the environment](../crates/iptv-player-mpv/src/mpv/handle.rs#L75) |
| S24 | Start with the device | Android TV boxes used as a set-top box, whose viewers expect to power on straight to live TV. Since Android 10 an app cannot start an activity from the background, including after boot, without an exemption such as the display-over-other-apps permission. Decide between a documented non-goal and an opt-in that explains the permission and how each target device grants it. Not applicable elsewhere. | Hidden `general.start_on_boot`, [Android background starts](https://developer.android.com/guide/components/activities/background-starts) |
| S25 | Update notice | About, on builds distributed outside a store: the APK, Linux packages and AppImage, macOS disk image and Windows installer. Check for a new release on request or by opt-in, say where to download it and never install silently. The check contacts an outside service, so it belongs in the privacy disclosures. Store builds leave updates to the store. | Hidden `general.check_updates`, [release channels](../README.md#install), [store privacy](store-privacy.md) |
| S26 | Erase all data | Data and privacy, behind administrative unlock. Remove playlists, credentials, favorites, history, settings and the PIN, optionally keep recorded media, and return to first run. Android and iPad can clear app data through the system; desktop has no equivalent, and no host has an in-app step. | No operation found. |

### Hidden schema keys

The rest of the schema is hidden without a recorded decision. The proposed
dispositions below let each key be accepted or rejected once.

| Disposition | Keys |
| --- | --- |
| Remove with scheduled recording | `recordings.lead_padding`, `recordings.trail_padding`, `recordings.reminder_wake` |
| Follow the system | `appearance.animated_scrolling`, `appearance.animated_transitions` (reduce motion) |
| Implement with an owning row | `general.reset_watch_time` (F25), `player.show_media_info` and `general.send_logs` (F27), `recordings.folder` (S22), `channels.playlist_opens` (S20), `history.keep_days` and `history.add_after` (S21), `logos.clear_cache`, `channels.strip_prefixes` and `channels.strip_suffixes` (W11) |
| Decide with a missing setting | `general.start_on_boot` (S24), `general.check_updates` (S25) |
| Planned with reminders (F16) | `recordings.reminder_action`, `recordings.reminder_lead`, `recordings.reminder_popup_timeout` |
| Already contextual | `channels.channel_sort`, `channels.group_sort`, `channels.manage_groups`, `epg.time_offset` |
| Already contextual, per playlist | `playlists.update_interval` and `playlists.update_on_start`: the playlist screen's Automatic refresh and Refresh on startup, which the runtime's library scheduler applies |
| Review individually | `recordings.format`, which applies to Record now, and the remaining `appearance.*`, `guide.*`, `player.*`, `channels.*`, `logos.*`, `history.*`, `vod.*`, `catchup.*`, `playback.*` and `recordings.*` keys outside the wired list |

## Key workflows

The feature rows are building blocks. Viewers judge Patina by these journeys,
each of which crosses several rows. A workflow passes on a host when a viewer
with that device’s habits completes it without a dead end, a placeholder or a
surprise. Patina supports W01–W14 at least in part today. W15–W21, under
[missing workflows](#missing-workflows), have no path on one or more hosts.

| ID | Workflow | What viewers expect by device | Rows and gaps today |
| --- | --- | --- | --- |
| W01 | First run to first picture | Phone: paste details or open a playlist from another app, in portrait. TV: enter a link and password without long D-pad typing. Desktop: paste, or drop a file onto the window. Everywhere: the guide says it is loading rather than looking empty while guide data downloads. Measure the time to the first picture. | F10, F21, F22, F01. The phone is landscape-only during setup, Android declares no open or share intent and TV has no assisted entry. |
| W02 | Turn on and watch | TV: the last channel plays within seconds, Ch± and number entry zap and Back reaches the guide. The screen never sleeps. Leaving follows the platform: directly on Android TV, a Yes/No popup on Tizen. Desktop and iPad on a stand: the same lean-back viewing. | F01, F04, F18, S02. Desktop and iPad have no sleep inhibition, exit confirmation is on for every Android host and neither Tizen host shows Samsung’s popup. |
| W03 | What’s on now and later | Phone: a Now and Next list, tap for full screen, a date control for other times. Tablet, desktop and TV: the grid with the preview. From a search result: Watch, Replay where catch-up allows, and Remind once reminders exist. | F01, F02, F07, F16. Remind me is a disabled row today; it becomes Planned with no entry point. |
| W04 | Leave and come back | Phone: Home enters PiP; a call pauses; headphones unplugged pauses. TV: Home stops playback and return reopens a fresh stream, with no PiP. iPad: Split View keeps playing. Desktop: minimize keeps playing; closing during a capture follows R01. | F04, F05, F19, R01. No media session, automatic PiP fires on Android TV, and today a desktop recorder can keep running with no window. |
| W05 | Replay and restart | Restart the current program, seek, then return to live, by touch, pointer, keyboard or remote. | F02, F03, F26. Back to live has no on-screen control. |
| W06 | Keep what I am watching | TV and desktop, and under More on Android handhelds: Record now from playback, see that it is recording and what will stop it, stop it or let the program end, and find it in Recordings, including a partial capture. Programs the viewer will miss are watched later through catch-up, not scheduled. | R01, R05, F18, S22. Desktop captures do not keep the computer awake, TV standby can cut a capture silently, and scheduled recording is removed (W21). |
| W07 | Watch a series through | Continue, resume, autoplay the next episode with a visible cancel, and remove a title that was abandoned. | F09, F25. No removal from Continue watching and no watched marks. |
| W08 | Share a TV with children | Set a PIN on the shared TV, unlock for a time window, relock and recover a forgotten PIN by remote. | F12, S13. The Tizen bridge offers no protection, so its listing must say so. |
| W09 | Something is wrong | A failed stream retries, then says why (account expired, connection limit, blocked region), offers a playlist refresh and, if that fails, a diagnostics export for the provider. | F04, F10, F24, F27. Account status and diagnostics are missing. |
| W10 | Set up a second device | Configure on the phone and move the setup to the TV or another computer. | F14, F22. Backup and restore depend on a document picker that many TVs lack. |
| W11 | Trim a large playlist | IPTV playlists often hold thousands of channels in hundreds of groups. Phone: hide unwanted groups and pick favorites in Manage. TV: do the same by remote in a few presses, with confirmation. Desktop: multi-select and reorder. Everywhere: strip provider prefixes and suffixes from channel names. | F08, S10. Prefix and suffix stripping is hidden, and bulk hiding by remote needs a journey check. |
| W12 | Fix a wrong guide | Programs an hour off, or missing on some channels: shift the offset, assign a guide channel or run a repair, starting from the affected channel on every input. | F11, S11. Offset, assignment and repair start in different places (group menu, channel menu, guide tools); check each is reachable from the channel on TV and phone. |
| W13 | Watch several channels | Desktop, Android tablets and Android TV: add tiles, choose the heard tile, enlarge one and return. Phones: two tiles in full-screen landscape, under More. The connection allowance is clear before a tile is added rather than after it fails. Not offered on Tizen; Planned on iPad. | F06, S07, F24. The viewer cannot see the account’s allowance. |
| W14 | Watch with a screen reader or captions | TalkBack, VoiceOver, Switch Access and desktop screen readers: tune a channel, hear what is on and operate the player without panels timing out. Caption users: captions stay on across channel changes and episodes, in a readable style. | F15, F28, S06, S17, S18. Every new stream starts with subtitles off, caption style is not configurable and only the play bar waits for a screen reader. |

### Missing workflows

Viewers on these hosts expect to complete these journeys and today cannot.
Each needs a decision to build it or to record it as a non-goal, so that the
documentation, support answers and store listings agree.

| ID | Workflow | What viewers expect by device | What is missing and rows |
| --- | --- | --- | --- |
| W15 | Listen with the screen off | Phone and tablet: radio or a news channel keeps playing with the screen locked or another app open, controlled from the lock screen and headset. | Playback stops in the background unless PiP is open, radio shows black video and there is no media session. Background audio needs a host decision on Android (a media foreground service) and iPad (the background audio mode, shared with picture in picture). F04, F05, F19, F23. |
| W16 | Watch on mobile data | Phone and cellular tablet: a warning before streaming on a metered network, and no surprise playback at launch. | No metered-network awareness, and resume on launch defaults on. S19, S20, F04. |
| W17 | Stay up to date | Sideloaded Android, Linux, macOS and Windows: learn that a new release exists and where to get it. | Packages come from GitHub Releases, so neither the app nor a package manager announces a new version. S25. |
| W18 | Hand over or reset a device | Every host, especially shared TVs and desktops: remove every login, history entry and the PIN in one step before giving the device away, after an optional backup. | No in-app step, and desktop has no system path either. S26, F14. |
| W19 | Power on to live TV | Android TV boxes used as set-top boxes: the box powers on into the last channel. | Start with the device is hidden, and Android restricts background activity starts. S24, F04. |
| W20 | Watch the phone’s channel on the TV | Phone and tablet: send what is playing to the TV. | Casting is removed as a non-goal. The supported path is Patina on the TV, set up through W10. F20. |
| W21 | Record a program I will miss | Every host: schedule a program or a series in advance and find it recorded later. | Removed on every host as a scope decision. Catch-up covers it only where the provider keeps an archive and its window has not passed; otherwise the journey is lost. It returns only as the explicit recorder mode described above. R02–R04. |

## Order for reviewing individual entries

| Order | Entries | Decision to make |
| --- | --- | --- |
| 1 | The device, window and input signals; R01–R05, S14, S22, F18, W06, W21; the hidden keys removed with scheduled recording; the migration | Accept the removal of scheduled recording, which hosts keep Record now and on what conditions, and how jobs, registrations and stored values are retired. |
| 2 | F05 and S07 (PiP on Android TV now), S02 (both Tizen hosts), F16 (placeholders and the failing Tizen NSH recording rows) | Fix what breaks a platform rule or fails today. |
| 3 | F03, F06, F09’s rail rule, F13, F14 with S15, S01, S03–S06, S08–S13 | Accept the device, window and input treatments and the behavior that replaces each removed setting. |
| 4 | W01–W05, W07–W20, F01–F02, F04, F07–F08, F10–F12, F15, F17, F19–F29, S16–S21, S23–S26 and the remaining hidden schema keys | Which journeys fail on which host, which missing operations and settings to add, and which hidden keys are implemented, follow the system or are dropped. |

For each entry, trace the visible control, shortcut, default key binding,
menu, settings row, remote action and runtime command, so a removed operation
disappears from all of them at once. When a capability disappears at runtime,
move focus to a valid control and keep access to existing data and
cancellation. Recheck authorization, provider capacity and availability at
execution.

The first concrete proposal is **F05**. Android uses the same device
capability to offer handheld and TV PiP, offers automatic entry on every
Android host and enters PiP when a TV viewer leaves. Google’s Android TV
guidance excludes media playback from permitted TV PiP uses. Keep manual and
automatic PiP on supported handhelds and manual PiP on desktop. On every
Android TV build, remove manual PiP from both control surfaces, remove the
automatic setting and refuse entry in the TV host. For Play distribution, also
declare the TV launcher’s activity without `supportsPictureInPicture`; an
activity alias cannot drop that attribute, so this needs a separate activity
declaration. Keep the in-app guide preview and verify the same player remains
mounted when switching between it and full-screen viewing.
[Android host report](../hosts/android/app/src/main/kotlin/com/hantekllc/iptvrs/host/PipHost.kt#L40), [TV entry on leaving](../hosts/android/app/src/main/kotlin/com/hantekllc/iptvrs/host/PipHost.kt#L142), [shared activity manifest](../hosts/android/app/src/main/AndroidManifest.xml#L56), [Patina PiP entry points](../crates/iptv-app/src/pip.rs#L73), [Android TV guidance](https://developer.android.com/training/tv/get-started/multitasking#usage-types)

The same entry-point check applies to recording. One device-class policy must
govern the play bar and control strip Record controls, the program menu rows,
the program details Record button and its key handling, the guide and player
Record key, the scheduled badges, the shared rail, the VOD browser’s own
navigation and the web host, so that a host without Record now shows none of
them and a host without a library shows no Recordings entry.
[Shared rail](../crates/iptv-presentation/src/browse.rs#L83), [VOD navigation](../crates/iptv-app/src/vod_view/browser.rs#L76), [details Record button](../crates/iptv-app/src/menu_view.rs#L1199), [details keys](../crates/iptv-app/src/input.rs#L872), [record key dispatch](../crates/iptv-app/src/input.rs#L968), [guide Record key](../crates/iptv-app/src/recordings_view.rs#L166), [scheduled badge](../crates/iptv-app/src/grid/cell.rs#L146), [web host](../crates/iptv-app/src/host/web.rs#L35)

## Review record

Five independent reviews challenged the removal and hiding proposal on
2026-11-24: three Claude reviewers focused on handheld and accessibility
viewers, TV and desktop viewers and platform policy, and code facts with an
implementation map, plus Codex and agy. The proposal changed as follows.

| Change | Raised by |
| --- | --- |
| The device class uses stable form-factor signals, the window size class and the connected input, because NSH’s Android idiom follows the window. | Handheld, code, Codex |
| Record now and audio delay are under More on Android phones as well as tablets, with free-space, metered-network and connection checks; none of the phone-only reasons were specific to phones. | Handheld, Codex, agy |
| Phones get two-tile multiview in full-screen landscape under More; iPad multiview is Planned until measured. | Handheld, Codex, agy |
| Panel timeouts stay under Accessibility until the platform’s accessibility timeout is honored (WCAG 2.2.1). Keyboard settings stay reachable while a keyboard is connected (WCAG 2.1.4). | Handheld |
| The scheduled-recording evidence is corrected: Low Power Standby is detectable, exact alarms are pre-granted through Android 13, inexact alarms can be an hour late, the six-hour limit counts background time, and an always-on desktop needs no privileges to stay awake. The removal stands as a scope decision, and the return criteria include a qualifying Android TV box. | TV and desktop, code, Codex, agy |
| Record now gains Android TV standby and storage checks, waits for F18 on desktop, keeps the Android capture watchdog or a replacement, and follows macOS close conventions. | TV and desktop, handheld, code |
| System PiP is removed on every Android TV build now, because automatic entry is live on TV today. | TV and desktop, Codex, agy |
| Both Tizen hosts need Samsung’s exit popup, which is new work on each. | TV and desktop, Codex |
| Retirement is a store migration that runs before any recorder, removes desktop login and wake registrations, keeps old backups restorable and defines each removed setting’s stored value. | Code, Codex, TV and desktop, agy |
| Corrections: `general.user_agent` is a default for portals, guide feeds, on-demand and recordings, not a duplicate; the catalog’s Sort does not persist; `parental.pin_input` decides what OK does; the stream limit is already per playlist; touch play bars use the seek steps; channel activation tunes the preview first. | Code, Codex, handheld |
| Missing entry points: the program details Record button and keys, the guide Record key, scheduled badges, the multiview action card, the VOD browser’s navigation and the failing Tizen NSH recording rows. | Code, Codex |
| External player stays in plain group options on Android TV, with a check that the loopback relay survives backgrounding. | TV and desktop, Codex |
| Consistency fixes: six labels, a split Tizen column, the summary table matched to the rows, the sleep timer kept on every host and parental controls kept at their usual depth on phones. | All |

Declined suggestions:

- **Keep exit confirmation on Android TV.** Google’s TV guidance asks apps not
  to gate exit, and Back from Patina’s guide already climbs through the groups
  and rail before leaving.
- **Keep scheduled recording behind runtime checks now.** The owner’s scope
  decision stands; the checks become the return criteria.
- **Add a separate Non-goal label.** Remove already covers an operation the
  host will not offer, built or not.
- **Add External player to every channel’s menu on Android TV.** It is a new
  operation, outside a removal pass.

## Verification

The core and presentation test suites, both portable WebAssembly builds,
the native app build and six targeted headless app scenarios passed.
The app scenarios covered TV, tablet and desktop scheduling, PiP capability
changes, iPad reauthorization after returning to the app, and desktop close
during recording. These checks simulate presentation and lifecycle events.
Hardware decoder capacity, OEM pickers, overnight wake behavior and the rendered
appearance of each platform need device evidence before closing those decisions.
The removal and hiding decisions, F18–F29, S16–S26, the W01–W21 gaps and the
review findings come from code search at `7580f8a4`, published platform
guidance and the five reviews, without new builds or device runs. The
following need device confirmation: whether an inexact Android alarm can start
a capture, Android TV Low Power Standby during Record now, Patina’s loopback
relay during an external-player handoff, screen and system sleep on desktop and
iPad, back to live on touch, subtitle selection on the Android player and the
bundled mpv’s deinterlacing default.
[Capabilities validation](capabilities.md#feature-availability), [platform validation](index.md)
