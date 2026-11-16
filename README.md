# Patina IPTV

![Patina — a weathered copper P and wordmark on a dark teal background](art/icon/banner.png)

**Keep watching. Find what’s next.**

Browse the guide over live TV, keep your favourites close, and make your
own IPTV playlists feel at home. Pick up a remote, tap the screen, or
reach for your mouse and keyboard.

For Android TV, Android tablets, Linux, macOS, Windows (experimental) and
iPad. Built in Rust with [Native Shell (NSH)](https://github.com/longbridge/nsh).

Inspired by TiviMate’s TV guide experience, Patina is an independent
project with its own design, code and artwork, and no affiliation with
TiviMate.

![The TV guide on Android TV, with the playing channel in the preview](docs/screenshots/tv-guide.png)

<table>
  <tr>
    <td><img src="docs/screenshots/tv-groups.png" alt="The menu and channel groups beside the guide"></td>
    <td><img src="docs/screenshots/tv-playback.png" alt="Full-screen playback with the programme card"></td>
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
a player that settles into a household's habits through its favourites,
custom groups and history.

## Features

- **Sources:** M3U playlists (by URL or file), Xtream Codes and Stalker
  portals, with several playlists browsable together.
- **Guide:** XMLTV EPG (plain or gzip) with automatic channel matching,
  per-channel guide choice, catch-up playback where the provider offers it.
- **Watching:** guide over live video, channel list beside the video,
  multiview (up to four tiles on desktop, two on Android),
  picture-in-picture on the desktops, track and picture-size choice.
- **Library:** favourites, custom groups, reordering, hiding, history and
  channel/programme search.
- **Movies and series:** browse provider catalogues by playlist and category,
  search and sort titles, choose seasons and episodes, resume unfinished items
  and continue to the next episode.
- **Parental controls:** protect channels, groups, movies and series with a PIN,
  timed or session unlocks, protected settings and a one-time recovery code.
- **Recording:** record live TV, schedule once, daily, weekly or by programme
  title, and keep captures running independently of the player window. Retained
  recordings support pause, seeking, resume and playback from the beginning.
  MPEG-TS and unencrypted MPEG-TS HLS use private local storage outside backups;
  [background execution depends on the host](#recordings-on-tv-tablet-and-desktop).
- **Resilient live playback:** streams that the provider cuts reconnect
  without dropping to an error screen.
- **Interface languages:** 11 language options with system-language detection
  and a live language picker; [coverage and translation guide](docs/localization.md).
- **Private by default:** the local catalog, including credentials, is
  encrypted with SQLCipher; HTTPS is the default transport. Whole-app
  backup and restore to a `.patinabackup` file.

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

Each release has a `SHA256SUMS` file and build provenance attestations.
With the [GitHub CLI](https://cli.github.com/) you can check that a download
was built by this repository's release workflow:

```sh
gh attestation verify Patina-<version>-linux-x86_64.AppImage --repo teng-lin/patina-iptv
```

[The release guide](docs/release.md#checksums-provenance-and-sbom) has the
stricter form, checksum verification and the SBOM. Releases up to v0.1.1
predate the attestations.

### Opening the macOS and Windows builds

The macOS and Windows packages are **previews** until platform signing is
configured: they are not signed with an Apple Developer ID or a Windows
code-signing certificate, and the macOS image is not notarized, so the
system warns before the first launch. Each release's notes say whether its
packages are signed. Verify the download as above first.

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
- The NSH fork checked out next to this repository at the commit recorded
  in [`nsh.rev`](nsh.rev), and its CLI built:

  ```sh
  git clone https://github.com/teng-lin/nsh.git ../nsh
  git -C ../nsh checkout --detach "$(cat nsh.rev)"
  (cd ../nsh && cargo build --release -p nsh-cli)
  ```

  NSH is a path dependency, so `Cargo.lock` cannot record its commit;
  `nsh.rev` does. CI checks out the same commit, and `tools/check.sh` fails
  when `../nsh` is at another one (`IPTV_ALLOW_NSH_DRIFT=1` allows it while
  you develop the fork). After `nsh.rev` changes, run
  `git -C ../nsh fetch origin && git -C ../nsh checkout --detach "$(cat nsh.rev)"`.

- **Linux:** GTK 4 and libmpv 2 (`mpv` on Arch, `libmpv-dev` on
  Debian/Ubuntu).
- **macOS:** Xcode command line tools and the pinned libmpv runtime below.
- **Android:** JDK 21, Android SDK (platform 35, build-tools 35.0.1), NDK 30.
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

Windows uses the NSH fork's Win32/Direct2D backend. Install Visual Studio
2022 Build Tools with **Desktop development with C++** and a Windows SDK,
Rust's MSVC toolchain, and Strawberry Perl on `PATH` (for bundled OpenSSL).
Keep the NSH checkout next to this repository at the `nsh.rev` commit as
above; its CLI is not needed for the Windows build.

From PowerShell:

```powershell
./tools/build-windows.ps1 -Check -Run
# Optimized executable and libmpv DLL in target/release:
./tools/build-windows.ps1 -Release
```

The script downloads Patina's checksum-pinned x64 libmpv runtime and copies
`libmpv-2.dll` beside `iptv-app.exe`. Keep these two files together when
running the app. Windows release tags and manual desktop releases also build
`Patina-<version>-windows-x86_64-setup.exe`; PRs run tests but do not build
the installer. The per-user installer adds a Start menu shortcut, bundles
libmpv and the VC runtime, and preserves settings and playlists on uninstall.
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

The Windows CI job and `-Check` command run what `tools/check.sh` runs on
Linux and macOS: lints, the tests of every library crate and of the app, and
the WebAssembly builds of the portable crates. `-Check` also needs Python,
to compare the NSH checkout with `nsh.rev`. A first build also
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

```sh
tools/fetch-mpvkit.sh            # libmpv xcframeworks, checksums pinned
cd crates/iptv-app
nsh package . --target ios-simulator -o ../../target/ios-sim
nsh package . --target ios -o ../../target/ios \
  --sign <signing identity SHA-1> --profile <app.mobileprovision>
xcrun devicectl device install app --device <id> ../../target/ios/Patina.app
```

Installing on a device needs Developer Mode on the iPad and a development
profile for `com.hantekllc.iptvrs`.

Desktop packages: `tools/ci/package-linux.sh` and `tools/package-macos.sh`.

### Try it with synthetic data

The repository has no real provider data. Local mock servers stand in:

```sh
python3 -I tools/make-iptv-fixtures.py --host 127.0.0.1:8089
tools/make-hls-fixtures.sh --serve &      # playlist, EPG and HLS on :8089
IPTV_RS_DEV_PLAYLIST=http://127.0.0.1:8089/iptv/playlist.m3u cargo run -p iptv-app
```

Use `10.0.2.2` instead of `127.0.0.1` from an Android emulator. Other
mocks: `tools/mock-xtream.py` (Xtream API), `tools/mock-live-cuts.py`
(live streams the server keeps cutting), `tools/make-track-fixtures.sh`
(multiple audio and subtitle tracks).

## Movies, series and parental controls

**Films** and **Series** in the navigation rail open the on-demand catalogue
provided by Xtream Codes or Stalker sources. Filter by playlist/category, search
by title and choose a sort order. Details show available metadata, seasons and
episodes; Continue watching restores saved progress. Episode playback can
continue into the next season. Availability and metadata depend on the provider.
M3U imports remain channel playlists; Patina does not guess a movie catalogue
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
  order across both catalogues, so each page sorts the enabled library.
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
policy, protected organisation or backups still requires administrative unlock.
Recovery resets the PIN and rotates the recovery code. Viewing locks do not
cancel recordings that were already authorized.

## Recordings on TV, tablet and desktop

Choose **Record** or **Schedule a recording…** in a programme's menu. The
programme details and full-screen controls also offer recording. On a TV, use
the remote's Menu/long OK or Record key; Up/Down chooses a form row and
Left/Right changes its value. Touch and mouse users have labelled −/+ buttons.
The navigation rail's **Recordings** entry manages current, scheduled and
finished recordings. Cancel removes a schedule; Stop retains captured media;
Delete removes media after confirmation. A dot marks recording programmes in
the guide, and the recording indicator opens management while watching.

Use **Repeat** to choose once, daily, weekly or series. Daily and weekly
schedules keep their local clock time across daylight-saving changes; series
rules match the programme title on the selected channel as guide data arrives.
An accepted individual job keeps its start/end times. Cancel repeating schedule
stops future occurrences without deleting finished recordings. If multiple
repeating schedules share the selected recording, this cancels all of them.

On Android, capture runs in a foreground service with a notification. Allow
precise wakeups in Recordings settings for scheduled starts; boot and package
updates restore pending wakeups. Android force stop prevents execution until
Patina is opened again, and Android 15 can limit background capture to six
hours in a 24-hour period. Device, network and storage availability still apply.

On desktop, closing the window keeps the process running when recordings or
repeating rules need it. **Start recorder at login** enables recovery after
sign-in; **Quit Patina** stops the recorder. Linux and macOS require an awake
computer. Windows includes an experimental scheduled-task wake hook, subject
to the system's wake-timer settings; it does not power on a shut-down computer.
iPad background recording is not supported.

Finished and partial recordings offer resume and playback from the beginning;
the player uses its normal pause, rewind, fast-forward and seek bar controls.
Interrupted jobs retain captured media and recover the remaining window when
possible; gaps and missed windows are reported. Reminders and network recording
destinations remain outside this feature. Provider accounts need enough
connections for watching plus recording. **Settings → Other → Streams at once** sets a local
ceiling (default one); known provider limits still apply. Accounts identified
as the same Xtream/portal account share their budget. M3U account associations
are unknown and are counted conservatively against other streams.

Media is stored beside the catalogue directory, in its `-recordings` sibling,
with a limit of 8 GiB per recording, four simultaneous recorders and a
conservative 32 GiB media budget. Recordings are not encrypted with the
catalogue and are excluded from portable and OS backups. Catalogue restore
clears recording schedules/history; existing media files remain in that
directory and are never overwritten by restored recording identities.

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
| `crates/iptv-core` | UI-independent state: guide model, focus, lists, panels, menus |
| `crates/iptv-nav` | Remote keys, long presses, key-to-action map |
| `crates/iptv-settings` | Settings schema, store and backup JSON |
| `crates/iptv-providers` | M3U parser, Xtream and Stalker clients |
| `crates/iptv-epg` | XMLTV parser and channel matching |
| `crates/iptv-guide` | Guide schedule and stream resolution |
| `crates/iptv-import` | Source and guide import pipeline |
| `crates/iptv-search` | Channel and programme search |
| `crates/iptv-library` | Channel organisation, custom groups, ordering, favourites and history |
| `crates/iptv-player-mpv` | libmpv playback adapter for desktop and iPad |
| `crates/iptv-runtime` | Headless application runtime, lanes, background tasks and scheduler |
| `crates/iptv-net` | HTTP adapter (reqwest + rustls) |
| `crates/iptv-store-sqlite` | Encrypted SQLite store |
| `crates/iptv-sqlcipher-init` | Native SQLCipher initialization barrier |
| `crates/iptv-app` | NSH app: screens, theme, video layer (libmpv on desktop and iPad) |
| `hosts/android` | Kotlin host with the Media3 player |
| `tools/` | Mock servers, fixtures, packaging and CI scripts |

All crates except `iptv-app`, `iptv-net`, `iptv-store-sqlite`, `iptv-sqlcipher-init`,
`iptv-player-mpv` and `iptv-runtime` are
sans-IO and build for `wasm32-unknown-unknown`.

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
bundle FFmpeg and mpv carry their licences too, and the
[source of those bundled libraries](docs/release.md#corresponding-source-for-bundled-media-libraries)
is published: the retained source archive of each platform's libmpv build
(named in the package's `SOURCE.txt`), and
`Patina-<version>-linux-native-sources.tar` beside the packages for the
Ubuntu libraries the AppImage bundles.
