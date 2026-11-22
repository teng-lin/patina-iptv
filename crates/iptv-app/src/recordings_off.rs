//! A build without the recordings screen (feature `recordings` off, as a
//! browser TV package builds it): nothing records, and no control,
//! menu row, rail item or key opens the screen or starts a recording.
//! What the guide, the menus and the player still ask about recordings
//! has its answer for that here.

use crate::Ui;

/// Nothing records.
pub fn recording_active(_ui: Ui) -> bool {
    false
}

/// No channel can be recorded: the Record controls and rows are left out.
pub fn available(_channel: usize) -> bool {
    false
}

/// No program is recorded or scheduled.
pub fn badge(_ui: Ui, _channel: usize, _program: crate::guide::Program) -> &'static str {
    ""
}

/// No recording is started from a program's menu or details.
pub fn remember(_ui: Ui, _origin: iptv_core::menu::Origin) {}
