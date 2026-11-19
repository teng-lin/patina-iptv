# NSH TV input

The remote decoder and Android scan-code table come from the `nsh-tv`
checkout at `910784e64d27726df99648c9883fd9f8778898c2`, under
[Apache-2.0](LICENSE). The Android transport lives in
`hosts/android/app/src/main/java/dev/nshtv/RemoteKeys.java`. NSH resolves
both Patina and this crate against the checkout pinned in `nsh.rev`.
Patina's `iptv-nav` owns its shortcuts, press timing and actions.
