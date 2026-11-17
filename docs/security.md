# Security model

## Catalog encryption

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

App-owned picked playlist copies also use SQLCipher, with document bytes stored in
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

Artwork passes through the public-address/DNS-pinning transport before NSH sees
bytes. Each response is capped at 4 MiB. Raster headers must describe nonzero
sizes of at most 8,192 pixels per side and 16,777,216 pixels total before reaching
a native decoder. This bounds one raster canvas, not all frames of an animation;
NSH's shared decoded-image budget remains responsible for cached frames. Malformed
or unsupported headers use the placeholder. SVG uses NSH's bounded parser and
rasterizer, with external files, remote references and embedded images disabled.

A public Xtream account keeps public-address checks enabled for its generated
stream URLs. A deliberately selected LAN account permits its own scheme, host
and effective port only; another origin does not inherit that exception. Stalker
uses its private-portal/owned-host policy. Playlist-supplied direct links remain
untrusted content. Redirect and subresource transport checks still apply. Explicit
HTTP media remains unencrypted on the network even though catalog storage is encrypted.
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
