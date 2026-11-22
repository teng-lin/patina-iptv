//! The state reported by a player.

/// The player state the host reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerState {
    Idle,
    Buffering,
    Ready,
    Ended,
}

impl PlayerState {
    /// The host's state names (`idle`, `buffering`, `ready`, `ended`).
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "idle" => Self::Idle,
            "buffering" => Self::Buffering,
            "ready" => Self::Ready,
            "ended" => Self::Ended,
            _ => return None,
        })
    }

    /// The host's name for it: what [`PlayerState::parse`] reads.
    pub fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Buffering => "buffering",
            Self::Ready => "ready",
            Self::Ended => "ended",
        }
    }
}
