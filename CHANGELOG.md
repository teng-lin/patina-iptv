# Changelog

All notable changes to Patina IPTV are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). While the version
is below 1.0, a minor release may change behaviour.

## [Unreleased]

### Added

- Releases publish the source of the media libraries bundled in the macOS disk
  image and the Linux AppImage (`Patina-<version>-<platform>-native-sources.tar`
  with a manifest), and the packages say where to find it.
- Releases carry build provenance attestations for every file and for
  `SHA256SUMS`, and a CycloneDX SBOM of the Rust dependencies.
- `docs/architecture.md`, `CONTRIBUTING.md`, `SECURITY.md` and this changelog.
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

### Fixed

- A release that is already public can no longer have its files replaced by
  rerunning the release workflow.
- Pull requests that change only Rust sources are compiled for Android.
- The Windows check runs the tests of every library crate.

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

[Unreleased]: https://github.com/teng-lin/patina-iptv/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/teng-lin/patina-iptv/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/teng-lin/patina-iptv/releases/tag/v0.1.0
