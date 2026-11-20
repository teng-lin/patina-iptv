# iOS simulator validation

Use only synthetic fixtures and a simulator created for the run. Do not reuse a
simulator that holds accounts, provider data or another run's state, and never
erase or delete a simulator you did not create.

## Entitlements

The simulator reads an app's entitlements from its executable's
`__TEXT,__entitlements` section, where Xcode's linker puts them, not from the
code signature. `nsh package --target ios-simulator` signs the app ad hoc and
links no such section, and an ad hoc signature carrying `keychain-access-groups`
does not launch (POSIX error 163). Without the entitlements the simulator's
Keychain refuses every item: the log shows `platform credential broker is locked
or unavailable` and adding a playlist fails with "The server refused access"
before any request reaches the server (#136).

So `iptv-app` links them itself on the simulator only
([`simulator_entitlements.rs`](../crates/iptv-app/src/simulator_entitlements.rs)):
`build.rs` writes `application-identifier` and `keychain-access-groups`, both the
`id` in [`nsh.json`](../crates/iptv-app/nsh.json), and a `#[used]` static puts
them in the section. Device builds are unaffected; they take their entitlements
from the provisioning profile when signed ([release](release.md)). No linker
flag or re-signing is needed. Check a package with:

```sh
tools/check-ios-simulator-entitlements.sh target/ios-sim/Patina.app
```

The iOS CI lane checks that the compiled simulator library (`libiptv_app.rlib`)
carries the section ([validation tiers](ci.md)). It links no app, so checking
the packaged app is a manual step: run the check above after packaging, as in
the next section.

## Build, install and launch

On macOS with Xcode, the pinned NSH CLI built from `../nsh-patina`
([NSH pin](nsh.md)) and the `aarch64-apple-ios-sim` Rust target:

```sh
root=$PWD
bash tools/fetch-mpvkit.sh
export IPTV_RS_MPVKIT="$root/target/mpvkit"
source tools/ci/locked-build-env.sh
NSH="$root/../nsh-patina/target/release/nsh"
(cd crates/iptv-app && "$NSH" package . --target ios-simulator \
  --icon icon-ios.png -o "$root/target/ios-sim")
tools/check-ios-simulator-entitlements.sh target/ios-sim/Patina.app

udid=$(xcrun simctl create "Patina Synthetic" \
  com.apple.CoreSimulator.SimDeviceType.iPad-Air-11-inch-M4 \
  com.apple.CoreSimulator.SimRuntime.iOS-27-2)
xcrun simctl boot "$udid"
xcrun simctl install "$udid" target/ios-sim/Patina.app
xcrun simctl launch "$udid" com.hantekllc.iptvrs
```

For a run with the synthetic playlist below, export its fixture variables
before `nsh package`. Pick the device type and runtime from `xcrun simctl list devicetypes` and
`xcrun simctl list runtimes`. When other runs share the Mac, give the simulator
and the mock server a name and port of your own.

## Synthetic playlist

The simulator reaches the Mac at `127.0.0.1`:

```sh
python3 -I tools/make-iptv-fixtures.py --channels 200 --host 127.0.0.1:8089
tools/mock-server.py 8089 fixtures/hls &
```

`nsh package` builds the release profile, which refuses playlists whose
streams are on a private host such as the mock server ("content stream URL
must name a public HTTP(S) host"). For a fixture run, package with the
development fixture hook ([security model](security.md)), which only a build
with debug assertions honors. It is compiled in, so export these before
packaging, then install and launch that package:

```sh
export CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true \
  IPTV_RS_DEV_PLAYLIST=http://127.0.0.1:8089/iptv/playlist.m3u
```

Add `http://127.0.0.1:8089/iptv/playlist.m3u` as a playlist link in the app.
It loads the 200 channels, and the mock server's log shows the request. The
app's own log (`xcrun simctl spawn "$udid" log stream --predicate
'process == "Patina"'`) must not report the credential broker. Even the
release package, without the hook, must reach the mock server: it then
reports that the playlist cannot be read, not that the server refused access.

Afterwards stop the mock server and remove the simulator you created:
`xcrun simctl shutdown "$udid" && xcrun simctl delete "$udid"`.
