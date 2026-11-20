//! The iOS simulator's entitlements, linked into the executable (#136).
//!
//! The simulator reads an app's entitlements from its executable's
//! `__TEXT,__entitlements` section, where Xcode's linker puts them
//! (`-sectcreate`), not from the code signature: an ad hoc signature that
//! carries `keychain-access-groups` does not launch. `nsh package --target
//! ios-simulator` signs ad hoc and links no such section, and without these
//! entitlements the simulator's Keychain refuses every item, so the database
//! key is never stored and adding a playlist fails before any request.
//! `build.rs` writes the plist from the bundle id in `nsh.json`;
//! `tools/check-ios-simulator-entitlements.sh` checks a packaged app has it.
//! Device builds take theirs from the provisioning profile when signed.

const PLIST: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/simulator.entitlements"));

/// The section's bytes, exactly the plist. `#[used]` keeps it although
/// nothing reads it.
#[used]
#[unsafe(link_section = "__TEXT,__entitlements")]
static ENTITLEMENTS: [u8; PLIST.len()] =
    *include_bytes!(concat!(env!("OUT_DIR"), "/simulator.entitlements"));
