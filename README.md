# Patina IPTV

![Patina — a weathered copper P and wordmark on a dark teal background](art/icon/banner.png)

**Keep watching. Find what’s next.**

Browse the guide over live TV, keep your favorites close, and make your
own IPTV playlists feel at home. Pick up a remote, tap the screen, or
reach for your mouse and keyboard.

For Android TV, Android tablets, Linux, macOS, Windows (experimental) and
iPad, and Live TV on Samsung Tizen TVs (experimental, see
[hosts/tizen](hosts/tizen/README.md)). Built in Rust with
[Native Shell (NSH)](https://github.com/longbridge/nsh).

Inspired by TiviMate’s TV guide experience, Patina is an independent
project with its own design, code and artwork, and no affiliation with
TiviMate.

![The TV guide on Android TV, with the playing channel in the preview](docs/screenshots/tv-guide.png)

<table>
  <tr>
    <td><img src="docs/screenshots/tv-groups.png" alt="The menu and channel groups beside the guide"></td>
    <td><img src="docs/screenshots/tv-playback.png" alt="Full-screen playback with the program card"></td>
  </tr>
</table>

![The guide on an Android tablet, with a live channel in the preview](docs/screenshots/tablet-guide.png)

<sub>All screenshots use the repository's synthetic playlist, guide and
test streams.</sub>

Patina provides no channels or content. You add the playlist or account
your IPTV provider gave you.

The [internal runtime API contract](docs/runtime-api-v1.md) documents source-scoped
inspection, guide repair, verification and undo for native application clients.
The [feature availability matrix](docs/capabilities.md) explains host limits,
player reports and background recording requirements.

### Why "Patina"

A patina is the finish that metal earns with age and handling, like the
green of weathered copper. Rust is iron's patina, and Patina is written in
Rust. The name also fits the app's look (dark ink surfaces with mineral
accents, and the metal P of its icon) and the way it is meant to be used:
a player that settles into a household's habits through its favorites,
custom groups and history.

## Features

- **Sources:** M3U playlists (by URL or file), Xtream Codes and Stalker
  portals, with several playlists browsable together.
- **Guide:** XMLTV EPG (plain or gzip) with automatic channel matching,
  per-channel guide choice, catch-up playback where the provider offers it.
- **Watching:** guide over live video, channel list beside the video,
  multiview (up to four tiles on desktop, two on Android),
  picture-in-picture on the desktops and Android phones and tablets, track
  and picture-size choice.
- **Library:** favorites, custom groups, reordering, hiding, history and
  channel/program search.
- **Movies and series:** browse provider catalogs by playlist and category,
  search and sort titles, choose seasons and episodes, resume unfinished items
  and continue to the next episode.
- **Parental controls:** protect channels, groups, movies and series with a PIN,
  timed or session unlocks, protected settings and a one-time recovery code.
- **Recording:** record live TV, schedule once, daily, weekly or by program
  title, and keep captures running independently of the player window. Retained
  recordings support pause, seeking, resume and playback from the beginning.
  MPEG-TS and unencrypted MPEG-TS HLS use private local storage outside backups;
  [background execution depends on the host](#recordings-on-tv-tablet-and-desktop).
- **Resilient live playback:** streams that the provider cuts reconnect
  without dropping to an error screen.
- **Interface languages:** 11 language options with system-language detection
  and a live language picker; [coverage and translation guide](docs/localization.md).
- **Private by default:** on desktop, iPad and Android the local catalog,
  including credentials, is encrypted with SQLCipher (browser TVs have no
  keystore; see [security](docs/security.md#samsung-tizen-host)); HTTPS is
  the default transport. Desktop and Android support whole-app backup and
  restore with a `.patinabackup` file. Backups keep the TV guide's setup and
  the app downloads the guide programs again after a restore.

## Install

Release builds are published on
[GitHub Releases](https://github.com/teng-lin/patina-iptv/releases):

| Platform | Package |
| --- | --- |
| Android TV and tablets (Android 8.0+) | `.apk` |
| Linux x86_64 (glibc 2.39+, GTK 4.14+) | `.deb`, `.rpm`, `.pkg.tar.zst`, `.AppImage` |
| macOS 14+ (Apple silicon) | `.dmg` (preview: ad hoc signed, not notarized) |
| Windows x64 (experimental) | `-setup.exe` installer (preview: unsigned) |
| iPad | Not released yet; [build from source](#ipad) |

The native Linux packages need the system's GTK 4 and GStreamer; they carry
Patina's guarded libmpv in a private directory.
The AppImage bundles them: GTK and GStreamer from Ubuntu 24.04, and Patina's
own libmpv, built with an LGPL-only FFmpeg from pinned sources. The macOS
disk image and the Windows installer bundle the same build of libmpv.

Release artifacts carry `SHA256SUMS`. The automated pipeline also produces build
provenance; historical exceptions below must not be treated as verified builds.
With the [GitHub CLI](https://cli.github.com/) you can check that a download
was built by this repository's release workflow:

```sh
gh attestation verify Patina-<version>-linux-x86_64.AppImage --repo teng-lin/patina-iptv
```

[The release guide](docs/release.md#checksums-provenance-and-sbom) has the
stricter form, checksum verification and the SBOM. Releases up to v0.1.1
predate the attestations. **v0.1.4 is also an exception:** its assets have release
membership attestations, but no release-workflow build provenance was found in the
November 17, 2026 audit. That verification command therefore cannot establish its
build origin. The separate `macos-v0.1.4` download is part of this exception.

### Opening the macOS and Windows builds

The macOS and Windows packages are **previews** until platform signing is
configured: they are not signed with an Apple Developer ID or a Windows
code-signing certificate, and the macOS image is not notarized, so the
system warns before the first launch. Each release's notes say whether its
packages are signed. Check hashes and, where available, build provenance first;
the v0.1.4 exception above remains unverified by the build-provenance command.

- **macOS (Gatekeeper):** drag Patina to Applications and open it once; macOS
  refuses. Then open System Settings, Privacy & Security, scroll to the
  message about Patina and choose **Open Anyway**. Alternatively remove the
  download marker in a terminal:
  `xattr -dr com.apple.quarantine /Applications/Patina.app`.
- **Windows (SmartScreen):** when "Windows protected your PC" appears, choose
  **More info**, then **Run anyway**.

## Build from source

### Prerequisites

- Rust (the version pinned in `rust-toolchain.toml`) with the
  `wasm32-unknown-unknown` target, and for Android also
  `aarch64-linux-android`, `armv7-linux-androideabi` and `x86_64-linux-android`.
- The pinned [Patina NSH fork and CLI](docs/nsh.md).
- **Linux:** GTK 4 and libmpv 2 (`mpv` on Arch, `libmpv-dev` on
  Debian/Ubuntu).
- **macOS:** Xcode command line tools and the pinned libmpv runtime below.
- **Android:** JDK 21, SDK platforms `android-35` (NSH packaging) and
  `android-36` (Gradle host), build-tools `35.0.1` and NDK `30.0.16248370`.
- **iPad:** Xcode. `tools/fetch-mpvkit.sh` downloads the libmpv build.
- `python3` and `ffmpeg` for the synthetic test streams.

Linux and macOS development builds also require Patina's native relay guard.
After installing the platform prerequisites, fetch the pinned runtime and set
its library paths in the shell used for Cargo (use `platform=macos` on macOS):

```sh
platform=linux
archive=$(python3 tools/ci/native_candidates.py "$platform")
python3 tools/mpv-runtime-unix/runtime.py fetch --platform "$platform" --archive "$archive" --destination "target/mpv-runtime-$platform"
export LIBRARY_PATH="$PWD/target/mpv-runtime-$platform/lib"
export LD_LIBRARY_PATH="$LIBRARY_PATH"
export IPTV_RS_MPV_LIB_DIR="$LIBRARY_PATH"
export DYLD_LIBRARY_PATH="$LIBRARY_PATH"
```

Published assets are verified by SHA-256. During an unmerged runtime update,
`gh` with read access to the repository's Actions artifacts fetches the exact
checksum-pinned candidate instead. HTTP playback fails closed when the native
guard is absent. Windows and iPad build scripts fetch the guarded runtime.

### Windows (x64, experimental)

Windows uses NSH's Win32/Direct2D backend. Install Visual Studio
2022 Build Tools with **Desktop development with C++** and a Windows SDK,
Rust's MSVC toolchain, and Strawberry Perl on `PATH` (for bundled OpenSSL).
The Windows build requires the pinned NSH checkout above. You can skip building
the NSH CLI.

From PowerShell:

```powershell
./tools/build-windows.ps1 -Check -Run
# Optimized executable and libmpv DLL in target/release:
./tools/build-windows.ps1 -Release
```

The script downloads Patina's checksum-pinned x64 libmpv runtime and copies
`libmpv-2.dll` beside `iptv-app.exe`. Keep these two files together when
running the app. Windows release tags, manual desktop releases and selected
packaging PRs build `Patina-<version>-windows-x86_64-setup.exe`. See the
[validation tiers](docs/ci.md) for selection rules. The per-user installer
adds a Start menu shortcut, bundles libmpv and the VC runtime, and preserves
settings and playlists on uninstall.
Windows installers are currently unsigned; no portable app ZIP is produced.
The runtime has a separate [dependency build workflow](tools/mpv-runtime/README.md)
with pinned sources, offline compilation, native component notices and retained
source archives. Installers include those notices and the exact source download
location. See [the release guide](docs/release.md#windows-installer-experimental)
for dependency updates and source retention requirements.

Settings and encrypted data live in `%LOCALAPPDATA%\Patina`; the database
key lives in Windows Credential Manager. The existing desktop keyboard,
fullscreen, multiview and picture-in-picture paths are enabled. PiP shares
the original player. Video is composed below the UI through mpv's software
renderer, with output capped at 1920×1080 and scaled by Direct2D. This is
CPU intensive; high-resolution playback and multiview need performance
validation. GPU rendering is not yet implemented on Windows.

The Windows PR job compiles the workspace test targets, runs Windows chrome
checks and builds the app. Nightly and release CI run the full Windows suite,
as does `-Check` by default. CI builds the portable WebAssembly targets on Linux.
`-Check` also needs Python to compare the NSH checkout with `nsh.rev`. A first build
compiles OpenSSL from source and can take several minutes. For direct Cargo
commands, put Strawberry Perl's `perl/bin` and `c/bin` directories, plus
`target/mpv`, on `PATH`.

To build the installer locally (requires Python and `cargo-about` 0.9.3):

```powershell
cargo install cargo-about --version 0.9.3 --features cli --locked
./tools/package-windows.ps1
./tools/ci/check-windows-package.ps1
```

The packaging script installs a checksum-pinned Inno Setup compiler under
`target/tools`, embeds Patina's icon, and includes dependency notices. The smoke
check uses a fresh synthetic profile and refuses to replace an existing installed
copy. Release CI uploads only the installer, alongside the other platform packages.

### Run

```sh
cargo run -p iptv-app            # Linux or macOS desktop
```

Android:

```sh
tools/sync-host.sh               # builds libnsh.so into the Android host
(cd hosts/android && ./gradlew assembleDebug)
adb install -r hosts/android/app/build/outputs/apk/debug/app-debug.apk
```

#### iPad

The iPad app is complete and runs on devices, but it is not yet
distributed. It plays video through the same libmpv player as the
desktops (MPVKit's LGPL build) and adds touch navigation of its own.

```bash
rustup target add aarch64-apple-ios-sim aarch64-apple-ios
tools/fetch-mpvkit.sh            # libmpv xcframeworks, checksums pinned
source tools/ci/locked-build-env.sh
nsh="$PWD/../nsh-patina/target/release/nsh"
(cd crates/iptv-app && "$nsh" package . --target ios-simulator \
  --icon icon-ios.png -o ../../target/ios-sim)
```

Installing on a device needs Developer Mode on the iPad and a development
profile for `com.hantekllc.iptvrs`. Set `signing_identity` to the signing
identity's SHA-1, `provisioning_profile` to the profile's absolute path and
`device_id` to the iPad's identifier. From the same shell, run the device build:

```bash
(cd crates/iptv-app && "$nsh" package . --target ios --icon icon-ios.png \
  -o ../../target/ios --sign "${signing_identity:?}" --profile "${provisioning_profile:?}")
xcrun devicectl device install app --device "${device_id:?}" target/ios/Patina.app
```

Desktop packages: `tools/ci/package-linux.sh` and `tools/package-macos.sh`.

### Try it with synthetic data

The repository has no real provider data. Local mock servers stand in:

```sh
python3 -I tools/make-iptv-fixtures.py --host 127.0.0.1:8089
tools/make-hls-fixtures.sh --serve &      # playlist, EPG and HLS on :8089
IPTV_RS_DEV_PLAYLIST=http://127.0.0.1:8089/iptv/playlist.m3u cargo run -p iptv-app
```

Use `10.0.2.2` instead of `127.0.0.1` from an Android emulator. The
fixture channels end; `tools/mock-server.py --loop` serves them as endless
live streams for long runs (see
[fixture streams](docs/android-validation.md#fixture-streams-ending-or-looping-live)). Other
mocks: `tools/mock-xtream.py` (Xtream API), `tools/mock-live-cuts.py`
(live streams the server keeps cutting), `tools/make-track-fixtures.sh`
(multiple audio and subtitle tracks).

## Movies, series and parental controls

**Films** and **Series** in the navigation rail open the on-demand catalog
provided by Xtream Codes or Stalker sources. Filter by playlist/category, search
by title and choose a sort order. Details show available metadata, seasons and
episodes; Continue watching restores saved progress. Episode playback can
continue into the next season. Availability and metadata depend on the provider.
M3U imports remain channel playlists; Patina does not guess a movie catalog
from channel names or URL extensions.

**Continue watching** and **Recently added**, below Live TV in the rail, list
movies and series from every enabled playlist in one place; the app still
starts on the guide. They appear whenever any enabled playlist has movies or
series, even while a live-only playlist is shown (Films and Series follow the
playlist on screen). Each row names its playlist, so titles that two
playlists share stay separate. Choosing a row opens the same details as Films
and Series: play, resume, seasons and episodes, on the same player. Leaving
and reopening a shelf returns to the same title, found by identity wherever
it now is, however deep: the shelf shows the page of 40 holding it. **Load
more** continues below and **Load previous** (or Up on the first title with a
remote) above, a page at a time. If the title has left the shelf, it opens at
the top; if the lookup fails, an error and **Retry** keep the title to find.

- *Continue watching* holds movies started for at least five seconds and not
  finished, the most recently watched first. A series appears once and
  follows the episode watched last. It stays while you are watching it, even
  if the next episode was stopped within five seconds, but a series only
  glanced at does not appear. When the episode watched last was finished, the
  row offers the next episode from its start. Finishing the final episode
  removes the series, and an older half-watched episode does not bring it
  back; playing one of its episodes again does.
- *Recently added* sorts by the date the provider reports adding a movie or
  last changing a series, newest first. Titles without such a date follow by
  name. The release year is never used as that date. No index serves this
  order across both catalogs, so each page sorts the enabled library.
- Both shelves can take a while on a large library or a long watch history,
  so the app reads them, and the place of a focused title, on a separate
  store lane from the one the guide reads from, and stops a read when the
  shelf closes.
- Disabled playlists are left out, and a deleted playlist's progress goes
  with it. Protected titles keep their row but not their poster or plot, and
  open only after the parental PIN. Playing always asks the provider for a
  fresh link.
- When something fails, **Retry** repeats exactly that: the same page (the
  titles already shown stay), the same title's details, or the same episode
  played the same way (resumed or from its start), after reading its details
  and link afresh and checking the parental lock and the playlist again. A
  playlist deleted and added again, or whose provider settings were edited,
  since the title was chosen is not used by Retry: nothing plays, and the
  title has to be chosen again.

Use **Block channel**, **Block group** or the on-demand block control to protect
content. The first protected action sets a four-to-eight-digit PIN and displays
a recovery code. Parental settings control live/on-demand protection, unlock
scope and whether opening general settings requires the PIN. Changing parental
policy, protected organization or backups still requires administrative unlock.
Recovery resets the PIN and rotates the recovery code. Viewing locks do not
cancel recordings that were already authorized.

## Recordings on TV, tablet and desktop

Choose **Record** on the play bar to start recording the playing live channel
immediately; the control turns red while capture is active. Press it again to
stop recording while playback continues.

Record is offered where you stay with the capture until it ends: on Android TV
and the desktops in the play bar, the program details and the Record key; on
Android phones and tablets under the play bar's **More** and in the program
menu. The iPad and the Samsung TV packages do not record. **Recordings** is in
the rail on Android TV and the desktops; on Android phones and tablets and on
the iPad it appears once the library holds a recording. Before a capture starts
Patina checks the 32 GiB library budget, the free space it needs (about 3.6 GB
an hour), a free provider connection and, on a phone or a tablet, a metered
network, and says what will stop it: on Android TV with Low Power Standby,
turning the TV off (the part recorded is kept); on Android 15 and later, the
six-hour background limit, after which Patina notifies you. A desktop keeps
the computer from idle sleep while it records. Closing the window while a
capture runs asks, on Windows and Linux, whether to stop recording and quit;
on macOS the recording goes on with the Dock icon and Quit asks first.

On a TV while watching, press Menu or long-press OK to open the control strip,
then choose **Record**.
In the guide, long-press a current program (right-click on desktop) to open
its menu and choose **Record**, without starting or changing playback.
Only a program on now can be recorded. Patina no longer schedules future
programs, repeats recordings or offers the manual recording editor. Recordings
and repeating schedules created with an earlier version stay in Recordings,
still run, and can be canceled there.
The navigation rail's **Recordings** entry manages current, scheduled and
finished recordings. Tap a finished recording to play or resume it. Back returns
to Recordings while playback continues; the playing row or **Return to playback**
returns to the player without reopening the stream. The guide's video preview
also returns to saved playback.

The row's cancel control cancels a scheduled recording, and its stop control
retains captured media. The trash icon on the right asks for confirmation.
Canceling leaves playback and the file intact. Confirming deletion automatically
stops that file if it is playing, then deletes it; other playback is unaffected.
Remote users can open secondary actions with Menu. A dot marks recording
programs in the guide.

Daily and weekly schedules created earlier keep their local clock time across
daylight-saving changes; series rules match the program title on the selected
channel as guide data arrives. Cancel repeating schedule stops future
occurrences without deleting finished recordings. If multiple repeating
schedules share the selected recording, this cancels all of them.

On Android, capture runs in a foreground service with a notification. Android
force stop prevents execution until Patina is opened again, and Android 15 can
limit background capture to six hours in a 24-hour period. Device, network and
storage availability still apply.

On desktop, **Quit Patina** stops the recorder, after asking while a recording
runs (see above for closing the window).
iPad background recording is not supported.

Scheduled recording is being removed. Upgrading from a version that had it
cancels its pending jobs and disables its repeating rules before any recorder
starts, keeps their recordings, and tells you once: Recordings lists what was
canceled until you close it. A capture the upgrade interrupted is kept as a
partial recording. The first launch also removes what earlier versions
registered: the Linux autostart entry, the macOS LaunchAgent, the Windows
"Patina Recorder" task with its wake timer, and Android's pending recording
alarm. A login launch from one of them removes them and exits.

Finished and partial recordings offer resume and playback from the beginning;
the player uses its normal pause, rewind, fast-forward and seek bar controls.
Interrupted jobs retain captured media and recover the remaining window when
possible; gaps and missed windows are reported. Reminders and network recording
destinations remain outside this feature. Provider accounts need enough
connections for watching plus recording. **Settings → Other → Streams at once**
uses the reported provider allowance automatically, or one connection when the
allowance is unknown. An explicit local ceiling and known provider limits still
apply. Watching and recording currently use separate upstream connections. Accounts identified
as the same Xtream/portal account share their budget. M3U account associations
are unknown and are counted conservatively against other streams.

Media is stored beside the catalog directory, in its `-recordings` sibling,
with a limit of 8 GiB per recording, four simultaneous recorders and a
conservative 32 GiB media budget. Recordings are not encrypted with the
catalog and are excluded from portable and OS backups. Catalog restore drops
the backup's recording schedules and history and keeps this device's
recordings, so their media in that directory stays playable.

## Keyboard

| Remote | Keyboard |
| --- | --- |
| D-pad, OK | Arrows, Enter |
| Back | Escape |
| Menu | `m` |
| Channel up / down | Page Up / Page Down |
| Play / pause | Space |
| Info, Guide | `i`, `g` |
| Previous channel | `p` |
| Rewind / fast-forward (catch-up) | `[`, `]` |
| Search | `/` or Ctrl+F (Cmd+F) |
| Volume, mute | Ctrl+Up / Ctrl+Down (Cmd on macOS), Ctrl+M |
| Full screen | F11 (Ctrl+Cmd+F on macOS) or double-click the video |

## Development

| Path | What |
| --- | --- |
| `crates/iptv-core` | Domain types and rules for guide, playback, recording and user data |
| `crates/iptv-presentation` | Shared feature state, display data and localization |
| `crates/iptv-nav` | Remote keys, long presses, key-to-action map |
| `crates/iptv-settings` | Settings schema, store and backup JSON |
| `crates/iptv-providers` | M3U parser, Xtream and Stalker clients |
| `crates/iptv-epg` | XMLTV parser and channel matching |
| `crates/iptv-guide` | Guide schedule and stream resolution |
| `crates/iptv-import` | Source and guide import pipeline |
| `crates/iptv-search` | Channel and program search |
| `crates/iptv-library` | Channel organization, custom groups, ordering, favorites and history |
| `crates/iptv-player-mpv` | libmpv playback adapter for desktop and iPad |
| `crates/iptv-runtime` | Headless application runtime, lanes, background tasks and scheduler |
| `crates/iptv-net` | HTTP adapter (reqwest + rustls) |
| `crates/iptv-store-sqlite` | SQLite store: encrypted natively, in-memory chunk files in the browser |
| `crates/iptv-sqlcipher-init` | Native SQLCipher initialization barrier |
| `crates/iptv-app` | NSH app: screens, theme, video layer (libmpv on desktop and iPad) |
| `hosts/android` | Kotlin host with the Media3 player |
| `tools/` | Mock servers, fixtures, packaging and CI scripts |

All crates except `iptv-app`, `iptv-net`, `iptv-store-sqlite`, `iptv-sqlcipher-init`,
`iptv-player-mpv` and `iptv-runtime` are
sans-IO and build for `wasm32-unknown-unknown`. `iptv-store-sqlite` builds
for it too, with SQLite compiled in through `sqlite-wasm-rs` and no SQLCipher.

Run every check before committing:

```sh
tools/check.sh
```

The app's scenario tests in `crates/iptv-app/tests/app/` run the whole app
headless, each in its own process with an empty `$HOME`.
`cargo test -p iptv-app --features test-support <name>` runs one.

Useful environment variables: `IPTV_RS_AUDIO=null` (mute),
`IPTV_RS_HWDEC` (mpv `hwdec`), `RUST_LOG=info` (player state, never stream
addresses).

[docs/contributing.md](docs/contributing.md) maps changes to subsystem owners,
contracts and validation commands.
[Accessibility journeys](docs/accessibility.md) documents semantic/layout checks and
repeatable platform screen-reader validation.
[docs/architecture.md](docs/architecture.md) explains the crate layering, the
sans-IO ports, the state and video-layer rules, the import pipeline and the
NSH boundary. [CONTRIBUTING.md](CONTRIBUTING.md) covers setup, checks and
commit style; the project rules for contributors and coding agents are in
[AGENTS.md](AGENTS.md). Release and signing setup is in
[docs/release.md](docs/release.md), and changes per release in
[CHANGELOG.md](CHANGELOG.md). The storage and transport security model is in
[docs/security.md](docs/security.md); to report a vulnerability see
[SECURITY.md](SECURITY.md).

`python3 tools/bump-version.py patch` bumps the version. A `v<version>`
tag builds and publishes the release.

## License

[AGPL-3.0-only](LICENSE). Bundled Roboto fonts are under the
[SIL Open Font License](crates/iptv-app/fonts/OFL.txt). Packages that
bundle FFmpeg and mpv carry their licenses too, and the
[source of those bundled libraries](docs/release.md#corresponding-source-for-bundled-media-libraries)
is published: the retained source archive of each platform's libmpv build
(named in the package's `SOURCE.txt`), and
`Patina-<version>-linux-native-sources.tar` beside the packages for the
Ubuntu libraries the AppImage bundles.
