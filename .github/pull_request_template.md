## Description

<!-- Briefly describe the changes and link any related issues or audit finding IDs. -->

## Pre-merge Checklist

- [ ] **Synthetic data only:** Confirmed that tests, fixtures, docs, logs, screenshots, and commit messages contain only synthetic/mock data. No real provider URLs, names, channel lists, or credentials.
- [ ] **Checks pass:** Ran `tools/check.sh` locally (formats, Clippy without warnings, deny, library tests, wasm builds, app tests, and CI test parity).
- [ ] **Portable core:** Portable crates (`iptv-core`, `iptv-nav`, `iptv-settings`, `iptv-providers`, `iptv-epg`, `iptv-guide`, `iptv-import`, `iptv-search`, `iptv-library`) do not depend on NSH or platform APIs and build for `wasm32-unknown-unknown`.
- [ ] **Independent design:** No code, artwork, icons, fonts, layouts, or wording copied from reference APKs. The original Patina palette and UX design are preserved.

### Device verification (if applicable)

*Required for changes to `hosts/android`, video layer (`video.rs`, `player.rs`, `tv.player`), Android input, or Android storage/threading.*

- [ ] Tested on Android TV (device or emulator profile `tv36`) using synthetic fixtures.
- [ ] Tested on Android tablet (device or emulator profile `tab36`) using synthetic fixtures.
- [ ] Tested on desktop (Linux / macOS / Windows) if applicable.
<!-- Describe which devices were tested and what scenarios were verified: -->
