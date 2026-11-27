# Security model

## Catalog encryption

This section describes the native platforms (desktop, iPad, Android). The
browser build has no SQLCipher; see
[the SQLite store in the browser](#the-sqlite-store-in-the-browser-nsh-build).

Production opens the catalog through the store worker and `credential_key::open_database`.
SQLCipher encrypts the complete database, including source credentials, stream and guide
URLs, headers, cached catalog rows, search indexes and WAL pages. The app's guide and
catalog scratch databases must use `Store::open_staging_connection` and
`Store::attach_staging_database`; a separately opened SQLCipher file needs the raw key
again when attached. Temporary SQLite sorting uses memory.

The random 256-bit database key is held by the platform credential broker:

- Linux: Secret Service, through stable keyring/dbus-secret-service libraries. The
  default persistent collection must be available and unlocked. A locked or missing
  broker produces a store-unavailable error; the app never falls back to plaintext.
  Unlock prompts have a maximum thirty-second wait rather than blocking forever.
- macOS and iOS: the application-specific Keychain entry. Apple builds use SQLCipher's
  CommonCrypto backend, avoiding another OpenSSL alongside MPVKit.
- Windows: a per-user Windows Credential Manager entry; app data is stored under
  `%LOCALAPPDATA%\Patina`, inheriting the user profile ACL. SQLCipher uses bundled
  OpenSSL. Reparse points in the app storage tree are refused.
- Android: a non-exportable Android Keystore AES-256 key wraps the random database key
  using AES-GCM. Only the authenticated ciphertext is persisted in `noBackupFilesDir`.
  The host installs the unwrapped key through JNI before mounting the Rust app. A
  missing key, malformed ciphertext, or authentication failure leaves storage closed.

Keys and keyring payloads are zeroized on drop and never included in Debug or errors.
The Linux/Apple/Windows broker stores a hexadecimal secret so KDE Wallet's UTF-8-only Secret
Service implementation is supported. First key creation is serialized by a filesystem
lock. The app's existing instance lock prevents competing production store workers.

On opening a legacy plaintext database, the store checkpoints committed WAL data,
exports into a private encrypted sibling, applies every pending schema migration there,
checks integrity, syncs it and atomically renames it over the original. An export or
migration failure keeps the original usable. A stale encrypted scratch copy from an
interruption is discarded before retry. No plaintext rollback copy is retained after
successful migration. This cannot erase plaintext copies in pre-existing backups,
filesystem snapshots or storage-device remapping; it protects newly copied live files.

On native platforms, app-owned picked playlist copies also use SQLCipher, with document bytes stored in
64 KiB chunks. The importer reads these through a bounded decrypted stream; no
plaintext replacement file is needed. Existing copies are sealed atomically during a one-time migration recorded inside
the encrypted catalog, including compressed files and prior failed-import leftovers.
Routine readers never reinterpret damaged ciphertext as a new plaintext document;
symbolic links are refused. A failed file pick never writes plaintext into app storage.

Portable backups are explicitly user-exported plaintext archives so they can be
restored on another device without its original keystore. `snapshot_to` uses
`sqlcipher_export` for that conversion because SQLite's backup API cannot cross the
SQLCipher codec boundary. A restored legacy/plaintext catalog is encrypted with the
receiving device's key before the restore is handed off as persistent pending state;
its copied playlists are sealed too. Creating or validating an explicitly requested
portable export/import can use private mode-0600 plaintext scratch files, which are
removed on completion or the next startup after an interruption. This is the same
plaintext exposure intentionally authorized for the portable archive, not normal
catalog or playlist persistence. Backup passphrases remain a separate feature.
Backups need the same care as the
provider credentials they contain. Losing the OS credential store makes its existing
encrypted catalog inaccessible; a portable backup can be restored on a new installation.
A portable backup leaves out the downloaded guide's programs and descriptions, which
the restored app downloads again; the guide sources, channel matches and repairs stay.
The snapshot is vacuumed, so the removed rows leave no free pages behind in the archive.

`Store::open` remains an explicit plaintext API for synthetic fixtures and portable
backup validation. Production entry points use `open_encrypted`. Headless app tests
inject a synthetic key through the nondefault `test-support` feature; release builds
have no environment-variable key override or plaintext bypass. A test-support build
without an explicitly installed synthetic key fails closed without contacting any
platform credential broker.

Validation includes wrong-key/plain-reader rejection, raw database and WAL scans,
legacy schemas, a committed uncheckpointed legacy WAL, migration failure preservation,
portable backup roundtrips, and encrypted scratch attachment. The ignored Linux broker
integration test must run inside an isolated DBus session and temporary keyring data
root with `IPTV_TEST_ISOLATED_BROKER=1`; it must never target the user's login keyring.

## Transport

Server and portal addresses without a scheme default to HTTPS. HTTP remains
available when the viewer explicitly supplies an `http://` address; onboarding
and saved-source details show “Not encrypted” and explain the exposure. The app
does not silently retry HTTPS sources over HTTP. Network redirects, HLS resource
URIs and DASH resource URLs that downgrade HTTPS to HTTP are refused.

Android's network-security configuration permits explicit HTTP because source
domains are user supplied and cannot be enumerated in a static manifest. It also
permits the app's loopback player connection. The Rust transport enforces the
per-request downgrade policy on every platform. Its TLS verifier uses public
Mozilla roots; Android's declarative trust configuration is not a replacement
for that native verifier.

Both Media3 and mpv use one bounded loopback transport per player. Only the
transport sends upstream identity headers: Authorization, Cookie,
Proxy-Authorization, SN and X-User-Agent. They stay on the original stream's
scheme, host and effective port, including redirects, HLS variant/segment/key/map
requests and DASH resources. Players receive no global identity headers. mpv also rejects decoder-selected top-level references outside the current relay capability before opening them, and disables automatic sidecar discovery. The
loopback URL contains a random capability and must never be logged, exported or
persisted. A replacement stream cancels its old transport and active requests;
the persistent video surface is unchanged.

Playback accepts HTTP and HTTPS resources; non-HTTP schemes and DRM key schemes are not supported by this transport. URL basic authentication is retained only for the original origin. Response cookies persist only on that exact origin, including its port. Cookie storage is capped at 64 entries and 16 KiB per player; oversized or excess new cookies are ignored while updates to existing cookies remain possible.

Progressive media is streamed with backpressure. Supported adaptive manifests
are HLS and DASH (with DASH HTTP clock/steering extensions explicitly refused); unknown textual playlist formats (such as ASX, PLS and
SmoothStreaming, sidecar SRT/TTML and multipart MJPEG) fail explicitly instead of letting a decoder follow URLs
outside the policy. WebVTT subtitle segments remain readable. This narrows
mpv's formerly unrestricted manifest auto-detection; additional formats require
a dedicated parser and tests before they can be enabled. Manifests are limited to 2 MiB,
rewritten output to 16 MiB, incoming request headers to 16 KiB, and simultaneous
connections to twelve per player. At most two manifest buffers are live across
all players; each manifest has a thirty-second body deadline and local writes
have a fifteen-second deadline. Redirects are limited to ten; connection and
idle-read timeouts are eight and fifteen seconds. Whole-stream duration is not
limited, because a live channel can run indefinitely.

OS backup is separate from user-initiated exports. Android disables backup and
explicitly excludes every storage domain from both cloud backup and device
transfer, including device-protected storage, plus legacy backup rules. iOS
marks the app data directory and temporary restore/backup directories as
excluded from OS backup. User-initiated backup files retain their existing
explicit export behavior.

Desktop startup requires an absolute HOME, makes app directories 0700 and files
0600, and sets a restrictive process mask before opening storage. Existing
files, including SQLite sidecars and copied playlists, are repaired at startup;
symbolic links in private app storage are refused rather than followed.


Windows flushes payload files before publication, but does not flush directory
handles (unlike Unix). Pending/previous restore directories still provide
restart recovery; directory metadata durability across sudden power loss is
not guaranteed. Legacy encryption closes its SQLite handle before replacing
the database on Windows, under the application's process-lifetime instance lock.


## Artwork and provider-selected destinations

Natively, artwork passes through the public-address/DNS-pinning transport
before NSH sees bytes; in the browser build it goes through the page's fetch
under the policy described in [Browser build of the app](#browser-build-of-the-app). Each response is capped at 4 MiB. Raster headers must describe nonzero
sizes of at most 8,192 pixels per side and 16,777,216 pixels total before reaching
a native decoder. This bounds one raster canvas, not all frames of an animation;
NSH's shared decoded-image budget remains responsible for cached frames. Malformed
or unsupported headers use the placeholder. SVG uses NSH's bounded parser and
rasterizer, with external files, remote references and embedded images disabled.

A public Xtream account keeps public-address checks enabled for its generated
stream URLs. A deliberately selected LAN account permits its own scheme, host
and effective port only; another origin does not inherit that exception. Stalker
uses its private-portal/owned-host policy. In native builds, an explicitly
entered loopback M3U address can provide streams from the same scheme, host
and effective port when the playlist response comes from that origin. Other
playlist-supplied destinations require public hosts. Redirect and subresource
transport checks still apply. Explicit HTTP media remains unencrypted on the
network even though catalog storage is encrypted.
The compile-time `IPTV_RS_DEV_PLAYLIST` fixture hook is absent from release builds.

## Export, recordings and local authority

Portable exports are unencrypted, including their credential-bearing database;
the export dialog says so before choosing the destination. Recorded media is
also unencrypted content in the user's selected directory. Catalog encryption
protects recording metadata, not video files. Completion receipts live inside the
encrypted database and bind a recording to its source incarnation and retained
file digest. The application does not persist stream URLs in those receipts.

Parental controls govern actions inside Patina; they are not an OS security
boundary against a user who can inspect process memory, replace app binaries or
manage the credential broker. Android private data/Keystore backup rules do not
make a portable export encrypted. Other OS backup tools may copy app files;
restoring encrypted files alone cannot reconstruct a lost platform credential.
Keep an intentionally exported portable backup separately and protect it as a
credential. No automatic telemetry or support upload is enabled.

## Samsung Tizen host

The Tizen host (`hosts/tizen`, `crates/iptv-web`) has no SQLCipher store and
no platform credential broker. Its account (Xtream server, username and
password, or the M3U address) is kept in the app's local storage on the TV,
in clear, readable by anyone who can debug the app on that TV; Favorites,
Recently watched and the stored guide are in the app's IndexedDB. Stored
guide feed addresses are kept only as hashes, and storage keys name a
playlist by a hash of its identity, but an M3U address with a short password
could be recovered from that hash by trying candidates.

Requests go through the browser. Content-supplied destinations (guide feeds a
playlist names, stream and logo addresses) are checked with
`iptv_providers::content_url` before use, and a redirect the browser followed
is checked again and may not downgrade HTTPS; the request is aborted when a
hop is refused. The browser resolves names itself, so a public name that
resolves to a LAN address is not detected, and the TV's player (AVPlay) and
image loader follow redirects of streams and logos without these checks.
Logos are not size-limited. The TV sends its own User-Agent on API requests.

### The SQLite store in the browser (NSH build)

The NSH build for browser TVs runs the same SQLite store
(`crates/iptv-store-sqlite`) compiled to WebAssembly, without SQLCipher. Its
database (sources with their credentials, the catalog with stream and logo
addresses, the guide, favorites and history) is kept in clear: in the app's
memory and, written by the host in 64 KiB chunks, in the app's IndexedDB,
readable by anyone who can debug the app on that TV. A browser app has no
keystore, so a key the app held itself would only obscure the data; the
store does not pretend to encrypt it. Freed pages are zeroed
(`secure_delete`), so a deleted source's credentials are not in the chunks
saved after the deletion. That cannot erase the superseded chunks the
browser's IndexedDB storage (LevelDB, and separate files for large values)
may keep on its flash until it compacts or deletes them, just as it cannot erase copies in backups or storage-device
remapping natively. Staging databases and a publish's copy of the database exist only in
memory, and only the store's own connection can open its database.

The host saves after a commit: it takes the changed chunks, which are a
snapshot of that commit (chunk bytes and the file's size) that later commits
do not change, writes them in one IndexedDB transaction and acknowledges the
take; the store gives out no other take until then, and a take dropped
unacknowledged returns its chunks to the next one. Storage therefore always
holds one commit's state: a crash or power loss can lose the last changes,
never mix two commits. At launch the store refuses to open while a chunk the
saved size calls for is missing. The Tizen host (`hosts/tizen-nsh`) loads
the chunks and the size in one read transaction before the module starts;
storage it could not read refuses the open rather than starting empty, which
the first save would make permanent. A chunk with wrong bytes of the right
length is still only caught by SQLite on use (no per-chunk checksum yet).

## Browser build of the app

The NSH build for browser TVs (`iptv-app`'s `host/web`, packaged by
`hosts/tizen-nsh`, experimental and not the default package) fetches
through the page, like the Tizen host above, so the native transport's
guarantees hold only in part:

- **Destinations.** A content-supplied address is checked with
  `iptv_providers::content_url` before the request, and the final address
  after a redirect is checked again: it must be public when the request was
  content-supplied or the redirect changed origin, and may not downgrade
  HTTPS. The page's fetch follows redirects before Rust sees them, so an
  intermediate hop reaches its address (a LAN address included) before the
  final one is refused; only its response is discarded. The browser
  resolves names itself, so a public name that resolves to a LAN address is
  not detected.
- **Headers.** A request carrying a header another origin must not receive
  (anything outside `iptv_providers::http::CROSS_ORIGIN_HEADERS`: portal
  identity, a playlist's API key) asks the page to refuse redirects
  altogether, since the page would forward the header to the new origin,
  where the native adapter drops it. A refused redirect surfaces as a
  connection failure. The page cannot send `Cookie`, `User-Agent` or other
  forbidden headers at all, so Stalker portals, which need them, do not
  work there.
- **Limits.** Byte budgets, the total time and the idle timeout are enforced
  by the runtime's network client and the web host; redirect counts and
  connection timeouts are the browser's own.
- **Storage.** The browser store is unencrypted (no key store), and settings
  and favorites last for the session until a private-files port exists.
- **Randomness and time.** Salts, recovery codes and SQLite's randomness
  come from the Worker's `crypto.getRandomValues`; local time from the
  page's time zone.
- **Package policy.** `hosts/tizen-nsh/app/config.xml` declares a CSP
  (Tizen's CSP mode): scripts only from the package, inline styles and
  `blob:` images for the renderer, `object-src 'self'` for AVPlay, and
  `connect-src`, `img-src` and `media-src` open, as providers are
  user-chosen hosts; the destination checks above still apply to every
  request the app makes. The player plays only sources without request
  headers (AVPlay cannot send the app's).

## Native assumptions and dependency exceptions

The libmpv client header requires process-wide `LC_NUMERIC=C`. Current Unix
initialization sets it once before the first mpv handle; that does **not** prove
other GTK or network threads cannot read the locale concurrently. A safe move
requires tracing toolkit startup on every native host. This follow-up does not
claim that `Once` establishes that missing precondition, and retains the current
behavior pending a separately tested host initialization change. The native
teardown ownership decision likewise remains evidence-led; it is not replaced
by an unproven detached destructor.

The `RUSTSEC-2026-0192` exception in `deny.toml` covers the transitive NSH font
parser's maintenance status. Owner: NSH dependency maintainer (repository
maintainers until assigned). Review by 2026-12-16, and whenever NSH changes;
remove it after the tracked skrifa migration and full text/render validation.
The pinned Rust, NSH, mpv/FFmpeg and platform dependency inventories are in
`tools/ci/pins.py`, the Cargo lockfiles and platform build manifests. An advisory
exception is scoped acceptance, not a claim that all native dependencies are
vulnerability-free.

## Casting on the local network

The desktop Cast sender gives the selected receiver a short-lived opaque HTTP address. The relay admits only that receiver's IP and requires a random capability for each media request. It keeps provider URLs and headers on the sender. The media and capability cross the local network in cleartext. Another sender that joins the Cast application can also read the capability. Device discovery and the Cast TLS certificate do not authenticate the TV's identity. Use a trusted local network and select the intended display.

The relay listens on the interface used to reach the TV, on TCP port 49152 by default. Set `PATINA_CAST_RELAY_PORT` to choose another port before starting Patina. See the [desktop firewall instructions](desktop.md#cast-live-tv) for a rule scoped to the receiver address.

AirPlay on macOS and iPad uses Apple's native video route picker and AVPlayer.
The public picker does not expose the selected receiver's IP to Patina. Its
relay therefore uses a random 192-bit capability and admits private IPv4 peers,
rather than claiming the Chromecast adapter's additional receiver-IP binding.
It binds a concrete private address on the Mac's primary physical interface or
the iPad's Wi-Fi interface, on an ephemeral port. A VPN-only or IPv6-only path
is unavailable. The capability and media use plain HTTP on the trusted LAN;
provider addresses, headers, playlist comments and rendition links stay behind
opaque routes. No provider address is passed to AVPlayer or the receiver.

AVPlayer may request a playlist while preparing the system picker. That request
waits behind the same admission gate as the receiver: the runtime validates
parental authority, reserves the account claim, retires local viewing inputs,
and only then permits an upstream fetch. While receiver approval is pending,
intent or authorization revocation cancels the handoff. Cancel restores local
playback; Play here retires the remote stream before reopening locally. Stop
revokes the relay and releases this app's player, without selecting a system
output or affecting another application's player. The persistent mpv surface
stays mounted throughout. Only live HLS is supported; this path does not remux
or transcode incompatible streams.
