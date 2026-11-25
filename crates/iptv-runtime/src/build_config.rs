//! Compile-time development options shared by the runtime and its hosts.

/// A development playlist the wizard's M3U form starts with, set when the
/// app is built (`IPTV_RS_DEV_PLAYLIST=http://10.0.2.2:8089/iptv/playlist.m3u`
/// for the mock server, `tools/mock-server.py`, from an emulator). Without
/// it, as in every release build, the link field starts empty.
#[cfg(debug_assertions)]
pub const DEV_PLAYLIST: Option<&str> = option_env!("IPTV_RS_DEV_PLAYLIST");
#[cfg(not(debug_assertions))]
pub const DEV_PLAYLIST: Option<&str> = None;
