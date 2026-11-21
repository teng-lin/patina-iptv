# Changelog

All notable changes to Patina IPTV are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). While the version
is below 1.0, a minor release may change behavior.

## [Unreleased]

### Added

- Desktop: a visible ⋯ "More options" on recordings rows and on the shown
  group in the guide's and the channel list's groups columns, so every
  secondary action has a pointer entry point beside right-click. A second
  player window's title says when it is muted. The desktop's wheel,
  shortcut, window and display rules are documented in `docs/desktop.md`.
- Multiview: an optional setting, **Sound follows the selected tile**
  (Settings, Playback, Player, Multiview; off by default), for viewers who
  prefer the focused tile to be heard (#179). With it on, moving the focus
  to another tile (the arrows, a tap or a click, or a tile added) makes it
  heard, and its speaker badge moves with it. A
  full-screen detour, enlarging, reducing and removing a tile keep the
  rules of #135. Desktop player windows and picture in picture keep
  **Listen to this**. Settings documents move to version 5; existing
  installs keep it off.

### Changed

- Multiview and desktop player windows: the viewer chooses which player is
  heard, and focus never moves the sound (#135). A tile's menu, its pointer
  chrome, a window's play bar and a compact picture-in-picture window's
  controls offer **Listen to this**; the heard tile shows a speaker and the
  others a muted one, and compact windows show theirs in a corner. Adding a
  tile or opening a window does not take the sound; removing or closing the
  heard one gives it back to the one heard before it, else to the main
  player. A full-screen detour, enlarging, reducing and resizing keep it.
- Shows: an episode ending no longer pulls the viewer out of where they
  went (#187). Full screen is unchanged. On the series' details, the next
  episode plays in the preview behind them (Play or OK takes it full
  screen), or, with autoplay off, waits there selected with "Ready for the
  next episode". Anywhere else, such as a list, the guide or settings,
  playback stops and the series' card and Continue watching hold the next
  episode. A PIN prompt for the next episode no longer pops up on its own.
- Back from a movie or an episode playing full screen now returns to its
  details instead of the guide, on every input (a remote's Back, Escape or
  Backspace, and the play bar's ‹, now labeled Movies or Shows): a movie
  on Play or Resume, a series with the played episode and its season
  selected. The title plays on, on the same player, and nothing is opened
  again. Back from the details then returns to the list or shelf the title
  was chosen from (Continue watching and Recently added included), on that
  title, and from there to the guide as before (#147). A Continue watching
  list or shelf is read again on the way, so a finished title leaves it. On
  a desktop in the operating system's full screen, the first Back from a
  movie, an episode or a recording now restores the window, as it does for
  live TV, and the next one returns to the details or to Recordings.
- Phone held upright, and other narrow portrait windows: the guide's header
  now stacks a preview as wide as the window over the focused program,
  instead of a thumbnail beside it. The channel list stays inside a narrow
  window, and where the video has no room beside the list it stays full
  screen under it instead of being laid out past the window's edge. On a
  phone the guide draws at 0.7 of the tablet's sizes, and the full-screen
  play bar's 44-point buttons keep one row on its side, the channel's name
  on a line above them. Phones run in landscape for now; portrait will get
  its own design (#168).
- When Patina plays the last channel on start and that channel is one of the
  playlist's favorites, Back from full screen now opens the Favorites list
  with the channel focused instead of All channels. A channel that is not a
  favorite of that playlist still returns to All channels.
- Desktop: closing the main window while recordings keep Patina running now
  also closes every player window and the picture-in-picture window, as
  quitting does; only the recorder runs on. On macOS and Windows a window
  left without a reachable title bar after a display is removed moves onto
  the nearest remaining display.
- Choosing a channel in the guide now watches it full screen in both guide
  layouts. One setting, **Keep the guide after choosing**, now applies to both
  layouts. It replaces the separate setting for the guide drawn over video,
  which used to be on by default. If you set either one, upgrading keeps the
  value you chose for the layout you use. If you never changed either
  setting, choosing a channel now closes the guide in the overlay layout too.
  Turn **Keep the guide after choosing** on to keep the old behavior.
  Settings files move to version 3.
- Empty Movies and Shows screens say why they are empty and offer one action:
  add a playlist, open the playlist's new **Movies and series** switch, retry
  a failed import, or refresh. A movie or series list that fails while live
  channels import is shown as a failure, not as an empty catalog.
  In a small window the message and its action take the whole width:
  the categories make way, and Close and the rail stay beside it, so
  **Add playlist** can be clicked or tapped at any text size.
- Catalog schema 24 records movie and series lists that failed to download.
  Downgrading an existing catalog is unsupported; retain a compatible backup
  before upgrading.
- Catalog schema 25 and settings version 4 store their names in US spelling
  (#139): favorites, guide programs and canceled recordings and imports keep
  their data when an existing catalog upgrades or an older backup restores.
  Settings and backups written before keep loading; a backup made before a
  settings version change now restores instead of being refused. Downgrading
  is unsupported; retain a compatible backup before upgrading.
- Tablets: scrolling the guide's channels or the channel list, or panning its
  timeline, cancels a preview that **Play the focused channel by itself**
  was about to start; a channel you tapped still plays. Panning into another day no
  longer scrolls the channels back to the focused one.
- Android: a call or a navigation prompt pauses playback and resumes it
  when it ends; another app's media and unplugged headphones or a
  disconnected Bluetooth headset pause it until you play again. A stopped
  channel never restarts by itself. All players share one audio focus.
  Patina no longer takes the audio back from another app until you play
  again, keeps it through the end of a call, gives it up when playback
  stops or goes to the background during a call, and holds none for an
  ended stream. In multiview the heard screen is the one paused and
  resumed. Anything you do to playback during a call (play, pause, seek,
  change channel) keeps the call's end from resuming over it.
- Guide on tablets: a drag at the top of the list, or one a scroll takes
  over, cancels a waiting preview too; and a
  pan from the program on air no longer cancels a channel you just
  tapped or restarts a canceled preview when you pan back.

### Fixed

- Movies and Shows: Back to a title beyond the first screenful of the list
  (from its details, or from the title playing full screen) now scrolls the
  list to that title, shown and selected, instead of showing the top of the
  list with the focus out of sight (#176).
- Shows: Back from an episode to an ordinary category now shows the new
  progress on the series' card at once, as movie cards already did, without
  the list being read again (#177).
- Cold start: the guide's first screen (the channels around the last one
  watched) shows as soon as it is read from the store, instead of only
  after the whole guide window was read, about half a second later with
  7,000 channels on an emulator (#160).
- iPad: a mostly sideways diagonal swipe in the guide, up to about 1.5
  across for every 1 down, now pans the timeline instead of scrolling the
  channels. Taps that wobble sideways by less than 10 points still play.
- With autoplay on, closing the menu that a long press or right-click
  opened on another channel in the guide no longer tunes that channel.
  Autoplay waits until you move the focus to another channel; choosing the
  channel, or a menu action that plays, still plays.
- Windows and iPad: right-clicking a channel's name in the guide (on iPad, a
  mouse's or trackpad's secondary click) now opens the channel's menu, as it
  does on Linux and macOS. Before, only programs and the groups columns
  opened a menu there. The menu offers nothing new on these platforms:
  Watch in new window stays Linux and macOS only.
- iOS simulator: the package now carries the Keychain entitlements the
  simulator reads, so it stores its database key and adding a playlist
  works without a linker workaround (#136).
- TV remote: a press of OK (or another key with a hold action) no longer
  acts on a different screen when the screen changes while the key is held,
  for example when a tap plays a channel. Holding a key also keeps the info
  card and the play bar on screen until the press is decided, so releasing it
  presses the card it was held on instead of reopening the card.
- TV remote: a key held through a PIN prompt no longer acts after the prompt
  closes, the OK that submits a PIN no longer opens the play bar when held,
  Back on a PIN prompt closes only the prompt (and a Back held from before
  the prompt does not close it), holding Menu or `M` no longer opens and
  closes the menu or the play bar repeatedly, and a hold on the play bar
  always returns focus to Play/pause.
- TV remote: a key held while touch opens and closes a PIN prompt no longer
  chooses or steps back on release; a Back pressed in a prompt that touch
  then closes no longer backs out of the screen beneath, and its repeats no
  longer stop playback; tapping another key while Back is held no longer
  lets Back's release dismiss the prompt; a channel change while OK is held
  on the info card no longer hides the card before the press is decided;
  and OK held on the play bar no longer opens the info card after Menu hides
  the bar. On a desktop keyboard, holding the Enter that closed a PIN prompt
  no longer acts on the screen beneath; a key the prompt took repeats again
  once pressed afresh (#145).
- Movies and Shows, TV remote: in a wider window an empty screen without an
  action (an import running) no longer leaves the remote on nothing. After
  **Refresh now** it moves to the categories; Right from them reaches Search
  and Sort, and Right reaches the action again once it shows.

## [0.1.5] - 2026-11-17

### Distribution

- Android distribution-signed APK/AAB for tablets and TVs; Linux packages,
  an ad hoc signed macOS preview and an unsigned Windows preview.
- Catalog schema 20 adds recording completion recovery. Downgrading an existing
  catalog is unsupported; retain a compatible backup before upgrading.

### Added

- Select live channels by number and clear viewing or search history from settings.
- Continue watching and Recently added in the navigation rail: movies and
  series from every enabled playlist in one list, read a bounded page at a
  time in a stable order, with the playlist named on each row. Continue
  watching shows one row per series and drops a series once its final episode
  is finished; Recently added uses the provider's added or last-modified date,
  never the release year. The guide remains the start screen. Both appear
  when any enabled playlist has movies or series, and reopening one returns
  to the same title by identity, however deep, on the page that holds it,
  with Load previous and Load more around it.
- A Retry control for failed on-demand lists, details and playback that
  repeats exactly the failed page, details or episode playback (keeping the
  episode and a restart), with fresh details, link and parental check; it
  never plays from a playlist deleted and re-added or edited since.
- Open the shared playback bar on Android TV with long OK or Menu and navigate
  its enabled controls with the D-pad. Up from the guide's first channel focuses
  the mini-player; OK returns the current stream to full screen.

### Changed

- Explain unavailable PiP/multiview actions and use current player and recording
  wake reports for feature availability, including awake-only recording limits.
- Complete the known interface-text catalogue in eleven languages, including
  settings help, guide/search/multiview hints and track names. New translations
  remain drafts pending fluent-speaker review.

### Fixed

- Recover completed recordings after interrupted publication, drain queued media
  writes before finalization, and avoid counting active recording bytes twice
  when checking storage reservations.
- Keep picture overrides attached to channel identity and recheck playback access
  on foreground return without treating resume as a transport failure.
- Keep auxiliary player controls independent when Movies or Shows is open in the
  main window, and skip abandoned queued database reads while preserving writes.
- Bound downloaded artwork dimensions before decoding, and restrict private
  Xtream playback exceptions to the selected account's origin.
- Keep guide pages, search selections and pending playback attached to their
  original content when playlists refresh, reorder or are replaced; failed
  guide reads remain retryable.
- Recheck parental access when recording requests are accepted and media or
  guide inspection results are returned, including after a concurrent relock.
- Bound cancelled guide inspection work and idle recording transfers, preserve
  signed provider links, and reject invalid guide identifiers and time values.
- Prevent stale picker and guide-repair completions from replacing newer work,
  and make Stop and sleep timers cancel pending playback and resume actions.
- Cancelled imports and stale work from a replaced source cannot publish a
  staged catalogue over committed data. Refused private backup storage is
  removed immediately without changing an existing archive.
- Disposing the final UI owner drains its native store connections and accepted
  small-file writes, including during unwinding, to avoid racing SQLCipher's
  process-exit cleanup.
- Keep enlarged PIN dialogs and recording actions reachable, show D-pad focus
  after touch entry, announce on-demand selection and recovery fields, and
  preserve a pending recovery code across a recording-window hide while requiring
  the PIN again before showing it.

## [0.1.4] - 2026-11-16

### Distribution

- Android tablet/TV packages include ARM64 and ARMv7 native libraries.
- Linux packages and the unsigned Windows preview were published under v0.1.4;
  the ad hoc signed macOS preview was published separately as macos-v0.1.4.
- Historical provenance exception: publication occurred outside successful
  completion of the automated publish job. The November 17 audit found release
  membership attestations for the v0.1.4 assets, not build-workflow provenance.
  These public assets remain immutable; a later release must use the complete
  verified publication workflow.

## [0.1.3] - 2026-11-15

### Added

- Provider movie and series libraries with search, category/source filters,
  sorting, seasons, episodes, Continue watching and cross-season playback.
- Parental PIN setup, change and recovery, protected channels/groups/on-demand
  content, configurable unlock scopes and native playback/mutation checks.
- Independent recording host with Android foreground execution and alarm/boot
  recovery, opt-in desktop login startup, daily/weekly/series schedules and
  interrupted-capture recovery. Host sleep and Android service limits apply.
- Native recording seek/duration, pause, durable resume, playback from the
  beginning and completion controls, sharing the persistent player surface.

- Linux and macOS prototype: open a live guide channel in an independent player window
  with the original player controls, while main playback continues.
- Independent player windows can switch to compact picture-in-picture and back
  without opening another stream, with their own pause, return and close controls.

### Changed

- README opens with Patina's banner and a new introduction.
- Patina's branded splash appears at launch while the app initializes.

### Removed

- Archived audit reports, implementation plans and foundation walkthroughs;
  the runtime API contract now stands alone.

### Fixed

- Touch player controls reserve room for every button, keeping optional
  picture-in-picture and recording controls clear of rewind and fast-forward.
- Auxiliary player windows keep channel browsing and programme clocks independent,
  target manual retries correctly, release refused sessions, and let the main
  player's audio controls reclaim sound.
- Popup players keep keyboard controls and overlays local, refresh channel lists
  by identity, and preserve pause after a refused channel switch. Hyprland PiP
  placement handles rotated displays and stream shapes without moving an
  already arranged window.
- Resuming the last channel at startup shows the compact channel banner,
  with the normal automatic timeout.

## [0.1.2] - 2026-11-14

### Added

- App-open DVR on TV, tablet and desktop: record-now and one-off schedules,
  timing/padding controls, recording badges, local playback, stop/cancel and
  confirmed deletion. Durable jobs reconcile interrupted/missed recordings;
  MPEG-TS/HLS capture shares provider connection accounting with viewing.
- Headless application runtime extracted into dedicated `iptv-runtime` crate behind explicit platform ports, owning background workers, scheduling, lanes, favourites, search, and backup orchestration.
- Portable channel organisation and guide state machine extracted into dedicated `iptv-library` crate.
- Native libmpv desktop and iPad player extracted into dedicated `iptv-player-mpv` crate with typed playback contracts.
- Android host hardening and CI smoke testing: emulator test harness for tablet (`tab36`) and TV (`tv36`) profiles testing boot, playback, refresh, activity recreation, and kill/recovery scenarios; JVM unit tests for playback transport and credential keys; and rejection of un-relayed video playback URLs.
- Accessibility descriptions for icon-only controls in the interface language.
- Releases publish the source of the media libraries bundled in the macOS disk
  image and the Linux AppImage (`Patina-<version>-<platform>-native-sources.tar`
  with a manifest), and the packages say where to find it.
- Releases carry build provenance attestations for every file and for
  `SHA256SUMS`, and CycloneDX SBOMs for Rust dependencies, bundled media runtimes, and the Android release classpath.
- Release workflow includes end-to-end dry-run rehearsal mode on disposable tags and draft retry body updates.
- Release rehearsals sign in a separate `release-rehearsal` environment without the release credentials: Android with a
  throwaway key made for the run, macOS and Windows as unsigned previews.
- Coverage floors enforced per library crate in CI, split fuzz targets with persistent corpus storage, and scaled scenario wait timeouts for slow CI runners.
- `docs/architecture.md`, `CONTRIBUTING.md`, `SECURITY.md`, issue/PR templates, CODEOWNERS, and audit documentation.
- Counts, dates and times follow the interface language: plural forms by
  each language's rules (`1 канал`, `2 канала`, `5 каналов`), weekday and
  month names in the language, and a 12-hour or 24-hour clock
  (**Settings → General → Clock**; by default the language's own convention).
- Adding and refreshing playlists and guide sources with their failures,
  backup and restore, organising channels and groups, custom groups, choosing
  a channel's guide data and the remote-key editor are translated in all
  eleven languages. Translation of the rest is still partial; see
  `docs/localization.md`.

- Unsigned macOS and Windows packages are announced as previews in the release
  notes. The Windows installer is signed through Azure Trusted Signing and the
  macOS app notarized when their credentials are configured.

### Changed

- Storage threading: UI reads use a dedicated read-only SQLite connection to bypass writer queues; custom group reordering runs in a single atomic transaction; and large guide schedules are built off the UI thread.
- Shared network runtime: single shared client across playback and imports, with cancellation of abandoned stream link resolutions and background small file writes.
- Transport and privacy hardening: only allowlisted headers are forwarded across foreign redirect origins; secrets and tokens are zeroed on drop; and private Stalker portals only access streams from their host.
- The Linux AppImage and the macOS disk image bundle Patina's own libmpv,
  built from pinned sources with an LGPL-only FFmpeg (`tools/mpv-runtime-unix`),
  instead of the build machine's Ubuntu or Homebrew libmpv and FFmpeg. Each
  runtime's dependency release retains its exact sources. The macOS runtime
  is built for macOS 14.0, the app's declared minimum, where Homebrew's
  bottles targeted the macOS of the build runner.
- The NSH fork commit is recorded in `nsh.rev`. Clone that exact commit;
  `tools/check.sh` refuses another one unless `IPTV_ALLOW_NSH_DRIFT=1` is set.
- The minimum supported Rust version is the pinned toolchain, 1.99.1.
- Automatic clock format follows CLDR 49 regional conventions (including
  Spanish in Mexico and numeric Latin America locales), with Unicode
  hour-cycle extensions and **Settings → General → Clock** overrides.
- The remote-key editor names the screen a binding applies in (`Guide`,
  `Player controls`) where it showed an internal name (`guide`, `controls`).
- Package scripts work without `rustup`, using the Cargo on `PATH`.
- Strict Gradle dependency verification enabled for the Android host.
- All tracked lines under `crates/` wrapped to under 200 bytes and the check exemption list eliminated.

### Fixed

- Refuse ambiguous mixed-case HLS URI attributes when quoting their rewritten
  values would create another URI marker for a player.
- Backup manifest parser rejects Windows reserved device names and trailing dots/spaces to prevent directory traversal.
- Guide imports track per-source budgets and retry transient network failures with exponential backoff.
- HLS rewriter correctly parses line endings with bare carriage returns.
- Overlay state tracking unified across back navigation to prevent stuck screens.
- Picked playlist and guide files are opened asynchronously without blocking UI interactions.
- A release that is already public can no longer have its files replaced by
  rerunning the release workflow.
- Pull requests that change only Rust sources are compiled for Android.
- The Windows check runs the tests of every library crate.
- Scenario test race conditions addressed with settled back-press transitions and explicit fixture storage synchronization.

## [0.1.1] - 2026-11-13

### Added

- Interface language selection: 11 language options, detection of the system
  language, and a picker that switches the interface without a restart.
- Windows x64 support (experimental), with a per-user installer that bundles
  libmpv built from pinned sources.
- Group browsing and scoped search in the multiview channel picker, which
  keeps its scroll position by the selected channel.
- On-screen Back controls on desktops and tablets.

### Fixed

- A failure to write local storage during an import is reported as a storage
  error.

## [0.1.0] - 2026-11-12

First public release, for Android TV and tablets, Linux and macOS.

### Added

- Sources: M3U playlists by address or file, Xtream Codes accounts and Stalker
  portals; several playlists browsable together.
- Guide: XMLTV guides, plain or gzip, with automatic channel matching, a
  per-channel guide choice and catch-up playback where the provider offers it.
- Watching: the guide over live video, a channel list beside the video,
  multiview, picture-in-picture on the desktops, and track and picture-size
  choice. Live streams that the provider cuts reconnect.
- Library: favourites, custom groups, reordering, hiding, history, and channel
  and programme search.
- Privacy: the local catalog, including credentials, is encrypted with
  SQLCipher; whole-app backup and restore to a `.patinabackup` file.
- Packages: a signed Android APK, Linux `.deb`, `.rpm`, pacman package and
  AppImage, and an ad hoc signed macOS disk image.

[Unreleased]: https://github.com/teng-lin/patina-iptv/compare/v0.1.3...HEAD
[0.1.3]: https://github.com/teng-lin/patina-iptv/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/teng-lin/patina-iptv/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/teng-lin/patina-iptv/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/teng-lin/patina-iptv/releases/tag/v0.1.0
