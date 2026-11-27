# App updates

Settings → About → Check for updates compares the installed version with the
highest stable semantic version published on GitHub. Drafts and prereleases do
not count. A newer local build is never offered a downgrade. The panel shows
installed/latest versions, the release date, the last successful check and
scrollable release notes. Failed checks remain failures, with the previous
successful result available; a missing package is reported separately.

Checks run in the foreground. The frequency choices are Never, On launch,
Daily, **Weekly (default)** and Monthly. Monthly means the same day of the next
calendar month, clamped to that month's last day. Manual checks work at every
frequency. A failed automatic check uses bounded retry backoff. Skip this version
suppresses its automatic notice; a manual check can still review it. Later
dismisses the notice without losing the result. A notice waits until fullscreen
playback and other panels have closed and can be reviewed with a TV remote.

GitHub requests use a separate client without provider headers or credentials.
Release notes render as text. Package links must match this repository's release
and the installation's platform, architecture and package format. Store Android
builds do not offer a direct APK. iOS/iPadOS, Tizen and terminal builds do not use
this checker. See the [privacy policy](privacy-policy/index.html).

## Download and installation

Every supported matching package has a manual download route. Installation
inside Patina requires additional authentication and platform capabilities:

| Installation | Behavior |
| --- | --- |
| Direct Android APK | Opt-in builds download a TUF-authorized APK, verify size and digest, then ask Android's installer to confirm its package, signing identity and higher version code. Store builds stay on their distribution channel. |
| Windows per-user setup | Opt-in builds verify TUF authorization, Authenticode publisher and timestamp before asking to close Patina and start the installer. Unsigned previews use manual download. |
| macOS app | Opt-in Developer ID builds use pinned Sparkle and its signed appcast for download, installation and relaunch. Current unsigned previews download the DMG manually. See [macOS configuration](macos-updates.md). |
| Linux AppImage | Opt-in builds verify the full image and ask before replacing a writable installation or saving elsewhere. The previous image remains available for recovery. Restart manually to use the replacement. |
| Linux DEB, RPM, pacman | Download the matching package and apply it using the system package tool. GitHub availability does not promise availability from a configured package repository. |

Authentication failure never enables installation. An explicit **Download
package** browser action remains available when an installer feed or appcast is
unavailable. Downloading through that action requires the viewer to install the
package using the platform's normal tools.

In-app downloads show progress and can be canceled. Closing the update panel
lets an accepted download finish; the runtime retains the verified file and
offers it again without downloading twice. Explicit cancellation removes partial
and retained stages. Stages are private temporary files and do not survive app
exit. Recording activity is checked again before an installer handoff. Deferring
installation preserves the staged file; it does not silently stop a recording.

Production activation still requires a published authenticated feed, trusted
keys, platform signing configuration and install-over-previous acceptance. The
repository's unsigned macOS/Windows previews do not become signed merely by
enabling this UI. See [release configuration](release.md#native-update-checks).

## Verification

Existing test executables cover actual loopback GitHub HTTP discovery, a signed
synthetic TUF repository, streamed download and byte/digest verification,
truncated/corrupt/oversized refusal, progress, cancellation, retained stages and
cleanup. Schedule tests cover frequency boundaries, persistence, skipped and
unseen versions, retry backoff and clock rollback. App journeys exercise About,
inline notes, close/reopen, failed-check recovery, delayed Android channel
reports, physical remote notice navigation, compact large-text layout and saving
Monthly. Native macOS fixture tests exercise the AppKit bridge's
generation fencing, restart approval and cancellation.

```sh
cargo test --locked -p iptv-runtime --features test-support --lib app_updates::
cargo test --locked -p iptv-app --features test-support --test app app_updates::
cargo test --locked -p iptv-app --features test-support --lib host::updates::view::
python3 tools/ci/test-macos-dmg.py
```

These fixtures prove the discovery and verified-download paths and native
contracts. Real signed upgrade acceptance must additionally use two packaged
releases and verify preserved synthetic library/settings data on each enabled
platform. Do not substitute a fixture pass for that release gate.
