# NSH pin

Patina uses the `patina` branch of the `teng-lin/nsh` fork. The exact commit in
[`nsh.rev`](../nsh.rev) pins the framework crates, CLI and Android Java runtime.

`tools/bootstrap.sh` (or `./tools/bootstrap.ps1` on Windows) handles both
first-time setup and `nsh.rev` updates automatically, refusing to touch a dirty
checkout unless `IPTV_ALLOW_NSH_DRIFT=1`. With `IPTV_ALLOW_NSH_DRIFT=1`,
bootstrap leaves a dirty or drifted checkout at its current revision and builds
`nsh-cli` from that revision without syncing `nsh.rev`; unset the override
before re-running bootstrap to sync the pin.

To set up the sibling checkout and build the CLI manually from the Patina
repository root (a manual `cargo build` does not write bootstrap's
`.patina-nsh-cli.rev` verification stamp; run `tools/bootstrap.sh` or
`./tools/bootstrap.ps1` normally before relying on `--check-only` or
`-CheckOnly`):

```sh
git clone --branch patina https://github.com/teng-lin/nsh.git ../nsh-patina
git -C ../nsh-patina checkout --detach "$(cat nsh.rev)"
cargo build --release --locked --manifest-path ../nsh-patina/Cargo.toml -p nsh-cli
```

When the pin changes, re-run `tools/bootstrap.sh` (or fetch the new commit and
rebuild the CLI manually):

```sh
git -C ../nsh-patina fetch origin "$(cat nsh.rev)"
git -C ../nsh-patina checkout --detach "$(cat nsh.rev)"
cargo build --release --locked --manifest-path ../nsh-patina/Cargo.toml -p nsh-cli
```

NSH is a path dependency, so CI and `tools/check.sh` use `nsh.rev` to check the
checkout. `tools/check.sh` requires a clean checkout at that commit. Set
`IPTV_ALLOW_NSH_DRIFT=1` only while developing the framework locally.

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
