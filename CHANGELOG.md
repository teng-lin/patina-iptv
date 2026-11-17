# Changelog

All notable changes to Patina IPTV are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). While the version
is below 1.0, a minor release may change behaviour.

## [Unreleased]

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
