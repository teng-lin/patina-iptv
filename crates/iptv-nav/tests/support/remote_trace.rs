//! Synthetic key phases shared by the bridge and NSH presenter tests.

use iptv_nav::{Phase, RemoteKey};

/// A lost Left release, followed by a new key and the late release.
///
/// Right must act once. The displaced Left cannot move the cursor or
/// open the groups, on release or when its old deadline passes.
pub const LATE_RELEASE: [(u64, RemoteKey, Phase); 5] = [
    (0, RemoteKey::Left, Phase::Down),
    (120, RemoteKey::Right, Phase::Down),
    (130, RemoteKey::Right, Phase::Up),
    (200, RemoteKey::Left, Phase::Up),
    (600, RemoteKey::Left, Phase::Repeat),
];
