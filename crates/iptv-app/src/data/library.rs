/// What the data layer tells the screens about the stored playlists.
#[derive(Clone, Copy)]
pub struct Library {
    /// The stored playlist on screen; `None` before one is (first run).
    pub shown: Signal<Option<SourceId>>,
    pub all: Signal<bool>,
    /// Refreshes of stored playlists since the app started.
    pub activity: Signal<BTreeMap<SourceId, Activity>>,
    /// Bumped when what is stored about the playlists changed.
    pub version: Signal<u64>,
}

impl Library {
    pub fn new() -> Self {
        Self {
            shown: signal(None),
            all: signal(false),
            activity: signal(BTreeMap::new()),
            version: signal(0),
        }
    }
}

pub use crate::runtime::data::library::Activity;
use iptv_library::SourceId;
use nsh_core::prelude::*;
use std::collections::BTreeMap;
