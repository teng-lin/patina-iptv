# First-release privacy evidence (#221)

Assessment date: November 24, 2026. Source baseline:
`0c76709d` with NSH `413cfb65bf87a7bd9937f223fdc56896d8a718c8`.
This is a source inventory and submission worksheet, not proof that a store
declaration was saved. Use **Refs #221** until the pending items below pass.
The maintained behavior summary is [Mobile store privacy](store-privacy.md).

## Evidence inventory

| Behavior | Source evidence | Data leaving Android tablet/TV and iPad |
| --- | --- | --- |
| Catalog, credentials, imported playlist copies | [Credential broker](../crates/iptv-app/src/host/native/credential_key.rs), [Android key installation](../crates/iptv-app/src/host/native/credential_key/android.rs), [security model](security.md#catalog-encryption) | Provider requests include the source credentials/headers required for that source. SQLCipher catalog and imported copies stay local. |
| Portal identity | [Portal request/profile/authentication builders](../crates/iptv-providers/src/stalker/mod.rs) | Viewer-supplied MAC, optional serial/device IDs/signatures and login/password; portal language/time zone, token and protocol profile fields. The app does not read a hardware MAC to fill these fields. |
| Playlists, guide, artwork, playback and recording downloads | [Request client](../crates/iptv-net/src/client.rs), [playback relay](../crates/iptv-net/src/playback.rs), [recording transport](../crates/iptv-net/src/recording.rs), [artwork policy](security.md#artwork-and-provider-selected-destinations) | Requests reveal source IP, User-Agent and requested resource to configured providers/content hosts. Provider identity headers stay on the authorized origin. Explicit HTTP is supported. Automatic guide refresh can make requests without a new tap; an active recording can continue after backgrounding on Android. |
| Viewing, progress, favorites, preferences and search | [Runtime search](../crates/iptv-runtime/src/search.rs), [local settings](../crates/iptv-app/src/wiring/settings.rs), [SQLite store](../crates/iptv-store-sqlite/src/lib.rs) | Stored history/search terms are not uploaded by these paths. A stream request still reveals the selected content to its host; that differs from uploading the history database. |
| Backups and device transfer | [Backup UI and warning](../crates/iptv-app/src/library_ui/backup.rs), [Android document transport](../hosts/android/app/src/main/kotlin/com/hantekllc/iptvrs/host/BackupComponent.kt), [Android exclusions](../hosts/android/app/src/main/res/xml/data_extraction_rules.xml), [iOS exclusions](../crates/iptv-app/src/host/native/platform_data.rs) | Android user export sends an unencrypted archive, potentially including credentials/history, to the chosen document destination. Private scratch copies can exist during that explicit operation. iPad backup/restore is not offered. Automatic private-data backup is excluded. |
| Recordings and external playback | [Recording host](../crates/iptv-app/src/host/native/recording_host.rs), [external player adapter](../crates/iptv-app/src/host/native/external_player.rs), [Android recording host](../hosts/android/app/src/main/kotlin/com/hantekllc/iptvrs/host/RecordingHost.kt) | Unencrypted recorded media stays in local/chosen storage; a user-selected external player or export destination can receive media where supported. Android Record now uses a foreground service; scheduled recording is unavailable. |
| Diagnostics, advertising and accounts | [Android runtime dependencies](../hosts/android/app/build.gradle.kts), [Media3 format listener](../hosts/android/app/src/main/kotlin/com/hantekllc/iptvrs/host/PlayerComponent.kt), [shipping features](../crates/iptv-app/nsh.json) | No Patina telemetry endpoint, advertising/analytics SDK or Patina account flow found in source. Media3's `AnalyticsListener` reports decoder formats into app state; its name does not imply upload. Local/platform logs exist. OS/store diagnostics follow platform settings, not an app upload flow. |
| SDKs and native libraries | [Android runtime classpath recipe](../hosts/android/app/build.gradle.kts), [Cargo lock](../Cargo.lock), [iOS runtime lock](../tools/mpv-runtime-ios/runtime-ios.json), [SBOM generator](../tools/ci/sbom.py) | Source graph: NSH, AndroidX/Media3 (Android), MPVKit/libmpv/FFmpeg (iPad), SQLCipher, Rust dependencies. Build-time Gradle analytics artifacts are not proof of runtime collection. Candidate classpath/Rust/media SBOMs and packaged privacy manifests still need reconciliation. |

This inspection establishes intended source behavior. It does not establish a
provider's retention policy, exhaustively measure network traffic, or certify a
different build's dependency graph. Synthetic device journeys must verify the
candidate rather than assume this inventory proves execution.

Platform source declarations are also part of this inventory:
[AndroidManifest.xml](../hosts/android/app/src/main/AndroidManifest.xml) sets
`allowBackup=false` and references both the
[legacy backup exclusions](../hosts/android/app/src/main/res/xml/backup_rules.xml)
and Android 12+ extraction rules. The
[network security config](../hosts/android/app/src/main/res/xml/network_security_config.xml)
permits loopback cleartext only; explicit user HTTP sources use the native Rust
transport. The [iPad plist input](../crates/iptv-app/nsh.json) permits HTTP loads.
The shared credential broker above uses `keyring::Entry` on iOS, with the
`apple-native` backend selected in [Cargo.toml](../crates/iptv-app/Cargo.toml);
there is no separate iOS credential-key module. Generated iPad plist and bundled
privacy-manifest checks remain package evidence owned by #222.

Android TV shares these data paths, but document import/export and external
players depend on available handlers. Verify those capabilities separately on
TV; a missing browser must display the policy address through the existing
fallback instead of making the policy unreachable in the app.

## Proposed store answers

Consulted current official [Google Data safety guidance](https://support.google.com/googleplay/android-developer/answer/10787469),
[Google User Data policy](https://support.google.com/googleplay/android-developer/answer/10144311)
and [Apple App Privacy guidance](https://developer.apple.com/app-store/app-privacy-details/)
on the assessment date. The mappings below are inferences from that guidance
and the source evidence above; no console answers were changed.

Google counts off-device transmission even to independently selected providers;
local processing alone is excluded. Expected user-directed transfers can be
exempt from sharing, but remain collection. Do not assume provider retention
is ephemeral. Reconcile all distributed tracks, including automatic refresh
and background recording, before finalizing optionality or sharing exemptions.

| Play field/category | Proposed answer or unresolved fact |
| --- | --- |
| Privacy policy | `https://teng-lin.github.io/patina-iptv/privacy-policy/` |
| Collect/share required data types | Collection is present when authenticating or requesting provider content. Do not select a blanket no-data answer. |
| Personal info → User IDs | Include provider account names/IDs; app functionality. Credentials can also occur in URLs/headers and exported archives. Resolve any additional authentication-data category in the live form. |
| Device or other IDs | Include configured portal MAC/serial/device identities; app functionality. User entry does not make their transmission local-only. |
| App activity → App interactions | Include provider-visible content requests; app functionality. Local history/favorites/progress do not themselves leave through the normal store path; an explicit backup export can include history. Assess that transfer separately. |
| In-app search history | No normal search-query upload found; explicit backup export may include saved terms. Verify the user-directed export treatment in the live form. |
| Files/docs and videos | Local import/recording is local processing; assess explicit backup/recording exports and external-player transfers with their actual destinations. |
| Location | No location API/use for deriving location found. IP visibility alone does not prove location inference; provider-side use is unverified. |
| Crash logs/diagnostics, ads/analytics | No automatic Patina upload or advertising purpose found. Confirm packaged SDK graph and platform-managed diagnostics scope before submitting. |
| All collected data encrypted in transit | **No** is the supported conservative answer: explicit HTTP requests remain possible. Catalog encryption does not change this. |
| Deletion/account creation | No Patina account creation. Local clear/remove controls and uninstall exist; no control deletes provider-held data. Do not promise a provider deletion mechanism. Owner must verify the form's deletion choices and support process. |
| Sharing, optional/required, ephemeral | Pending owner reconciliation for all supported flows and distributed versions. Provider retention is unknown; no ephemeral assertion is verified. |

Apple's collection threshold concerns developer/integrated-partner access
beyond serving a real-time request. Its definition of partners refers to
integrated vendors, not every host a viewer independently enters. Therefore
the source-level proposal is **Data Not Collected**, with **no tracking**,
conditional on the packaged SDK/privacy-manifest inventory and confirmation
that there is no developer/partner retention path. Independently chosen
provider requests still belong in the public policy. Do not copy Google's
form answer into Apple's form or declare this proposal submitted. An SDK
privacy manifest, required-reason API declaration and export-compliance answer
are separate artifacts; none substitutes for App Privacy answers.

## Policy deployment and source link inspection

- Anonymous HTTPS GET of the public URL on the assessment date returned
  **HTTP/2 200**, `text/html; charset=utf-8`, without authentication or a redirect.
  The deployed page was byte-identical to the baseline repository page:
  SHA-256 `1b974d7612ce720a0dbfe86af9863f52f135980a9929bf8e8411267d961044ef`.
  This branch clarifies portal identity transmission and dates the policy;
  check the deployed revision again after Pages publishes the merge.
- Source inspection: [settings code](../crates/iptv-app/src/screens/settings_view.rs)
  renders **Settings → About → Privacy policy** for both native platforms and
  requests that exact HTTPS URL through NSH's `open_url`. If the request reports
  failure, the UI displays the address. This is not an observed platform browser
  launch on a release build.
- The existing [headless normal-entry scenario](../crates/iptv-app/tests/app/personalization_appearance.rs)
  exercises navigation and browser failure. It does not prove an Android/iPad
  release browser opened or that either store listing links correctly.

Local validation on the assessment date used pinned Rust 1.99.1 and NSH at the
revision above. Core/presentation tests, both `wasm32-unknown-unknown` builds
and `cargo build -p iptv-app` passed. The existing headless scenario passed via
`cargo test -p iptv-app --features test-support --test app privacy_policy`
(one test). These results do not replace release-device or console evidence.

## Candidate, device and console status

| Evidence class | Verified result / remaining work |
| --- | --- |
| Source | Inventory above at the stated baseline; no source behavior changes in this lane. |
| Release packages | Pending final Android AAB/APK SDK inventory and iPad bundle/manifest audit. #220 owns Android optimized artifact verification; #222 owns iPad packaging inspection. Any earlier release package must retain its own source SHA/hash and cannot stand in for the candidate. |
| Emulator/simulator | Pending normal-entry candidate checks on `tab36`, `tv36` and iPad of About → Privacy policy, plus import/play/refresh/record/export observations using synthetic data. On TV verify D-pad access and the no-browser address fallback. A headless UI scenario is a separate evidence class. |
| Physical Android tablet/TV and iPad | Not verified by this lane. Repeat the supported journeys on authorized physical hardware, including TV-specific picker/external-player availability and policy fallback; record model/OS, source SHA and artifact hash separately from emulator results. |
| Store console/listing | Access blocked before either console page opened: browser attachment requires a desktop-app network-policy/update change. This does not establish whether the account has store permission. No designated policy field, Data safety answer, App Privacy answer or live listing was read, saved or published. |

## Owner follow-through

1. Restore console browser access or provide an authorized purpose-built store
   connection. Check the selected app identity privately, then enter/verify the
   policy URL in Play **App content → Privacy policy** and App Store Connect
   **App Privacy → Privacy Policy URL**, including required localizations. Verify
   that the policy's app name matches each listing; the package namespace alone
   does not establish the store's developer identity.
2. Reconcile the worksheet with exact submitted builds, all active Play tracks,
   runtime SBOMs, iPad embedded privacy manifests and observed synthetic traffic.
   Resolve provider retention/sharing/optionality and support/deletion facts,
   any IP/time-zone location inference, authentication-data categories and
   export treatment for history, searches, files and videos. Save drafts for
   facts that remain undecided.
3. Finish Data safety and App Privacy in the authorized consoles, verify their
   previews/listing links and capture only sanitized field/status/date evidence.
   Store credentials, signing identities, account screens and provider payloads
   must stay out of Git, public PRs and reviewer inputs.
4. After the policy update deploys, repeat anonymous reachability/content checks
   and normal-entry Android tablet/TV and iPad browser checks on the submitted
   artifacts, including the no-browser address fallback on TV.
   Record emulator and physical results separately. Close #221 only when both
   stores' final answers and links agree with those results.
