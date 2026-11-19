# NSH pin

Patina builds against the upstream `longbridge/nsh` revision in
[`nsh.rev`](../nsh.rev). Use a sibling checkout at `../nsh-patina`. The
pin covers the framework crates, CLI and Android Java runtime. The current
migration uses a source commit. When NSH publishes the release with these
changes, resolve its tag to a commit and use that commit in `nsh.rev`.

The [vendored TV input adapter](../vendor/nsh-tv/README.md) consumes NSH's raw
key events and translates the Android TV scan codes that need transport.
[Raw key names for Android OK](https://github.com/longbridge/nsh/pull/2382),
[root dispatch](https://github.com/longbridge/nsh/pull/2414) and
[Android Back release](https://github.com/longbridge/nsh/pull/2344) support
Patina's TV input on upstream NSH. The Patina Android host owns the persistent
Media3 surface. When the software keyboard closes, Patina restores focus to
its remote listener. `iptv-nav` owns the app's navigation and press rules.

For dependency changes, follow the [runner lock update](release.md#locked-package-builds)
procedure and verify the Android TV input on a native host. Record both Patina
and NSH commit IDs in release candidate evidence. When reverting a pin, revert
its dependent application and runner changes together.
