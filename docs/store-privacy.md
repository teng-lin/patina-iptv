# Mobile store privacy disclosures

The public [Patina IPTV Privacy Policy](https://teng-lin.github.io/patina-iptv/privacy-policy/)
is published from [this repository](privacy-policy/index.html) by GitHub Pages.
Android and iPad expose that URL at **Settings → About → Privacy policy**. Use
the same URL in Play Console and App Store Connect.

## Release behavior

| Data or feature | Android tablet and TV | iPad |
| --- | --- | --- |
| Playlist and provider credentials, headers and guide sources | SQLCipher catalog, with key protected by Android Keystore. Provider authentication and requests go directly to the configured service. | SQLCipher catalog, with key protected by the Apple Keychain. Same direct requests. |
| Playlists, guide, artwork and playback | Imported documents and catalog data are local. Requests go to the configured hosts or content hosts for streams, guide data and artwork. Media3 receives media through an app-local relay. | Same content requests. MPVKit receives media through the app-local relay. |
| Preferences, favorites, search history, viewing history and playback progress | Stored locally. The watch history database and search terms are not sent to providers. A provider can infer viewing from stream requests. | Same. |
| Backups and restored data | User-requested exports are unencrypted and may contain provider credentials and history. Android uses the system's document picker. The selected storage provider may receive the file. Automatic app-data backup and device transfer are excluded. | In-app backup and restore are unavailable. Private app data is excluded from automatic system backup. |
| Recordings | Media on device, with optional user-selected exports and Android alarm and foreground-service support for scheduled recordings. The app fetches media from the selected provider. | Media on device where supported. No Android-style background recording service. |
| Diagnostics and accounts | Local app and platform logs. No automatic Patina diagnostics upload, analytics, ads or Patina account. Google may collect diagnostics under its own OS or store settings. | Same app behavior. Apple may collect diagnostics under its own OS or store settings. |
| Bundled components | NSH, AndroidX/Media3, SQLCipher and Rust libraries. See the Android classpath and Rust SBOMs for the release graph. | NSH, MPVKit (including libmpv and FFmpeg), SQLCipher and Rust libraries. See the verified MPVKit inventory and Rust SBOM. |

The [security model](security.md) documents the encryption boundaries, explicit
HTTP support and export handling. The policy covers transfers to configured
providers, provider-selected hosts, external players and storage destinations.

## Listing decisions to verify on the submitted builds

- **Google Play Data safety.** Local-only history, preferences and cached data
  are outside data collection. Off-device provider requests can carry user IDs,
  account credentials, connection headers and signals about which content
  the user requested. Review the applicable categories for every track and
  feature. Google's exception for an expected user-initiated transfer applies
  to sharing and does not remove collection from the form. Review automatic
  guide refresh and scheduled recording against that distinction. Provider
  retention is unknown, so do not assume ephemeral processing. Explicit HTTP
  is supported, so the app cannot declare that all data is encrypted in
  transit. Patina has no account creation or deletion flow.
  [Google Play Data safety guidance](https://support.google.com/googleplay/android-developer/answer/10787469)
- **Apple App Privacy.** Source review found no Patina-operated data collection
  or tracking by bundled components. Confirm that assessment against the
  packaged iPad app and its SDKs. Apple asks about data accessible to the
  developer or integrated third-party partners beyond the time needed to
  service a request. The viewer's independently chosen provider can retain
  its own requests and is covered in the privacy policy.
  [Apple App Privacy guidance](https://developer.apple.com/app-store/app-privacy-details/)

Verify the deployed policy URL from each app and listing. Keep release
artifact, SDK inventory and console evidence with the store submission record.
