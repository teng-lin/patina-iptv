//! Application results applied by the shell through one event sink.
use crate::model::{Activity, BackupProgress, SaveFailure};
use iptv_core::guide::Schedule;
use iptv_library::SourceId;
use std::rc::Rc;

/// The shell reconciles a catalog swap differently from an organization edit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleCause {
    /// The replacement catalog's rail availability, needed during reconciliation.
    Catalog { has_vod: bool, any_vod: bool },
    /// The channel catalog stayed the same.
    Organization,
}

pub enum RuntimeEvent {
    ResumeChannel(usize),
    SchedulePresented(Vec<(&'static str, std::time::Duration)>),
    CatalogChanged,
    EarlyResume {
        source: Box<super::model::StoredSource>,
        done: futures_channel::oneshot::Sender<()>,
    },
    Message(String),
    SettingsSaveFailed(SaveFailure),
    BackupProgress(BackupProgress),
    FavoritesChanged {
        channels: std::collections::BTreeSet<usize>,
        /// The source on screen has movies or shows (Movies, Shows).
        has_vod: bool,
        /// Some enabled source has movies or shows (the cross-source
        /// Continue watching and Recently added shelves).
        any_vod: bool,
    },
    FavoriteOrderChanged,
    RefreshProgress(SourceId, Option<Activity>),
    BrowsingAll(bool),
    ShownSource(Option<SourceId>),
    /// The runtime installed `current`; old row indices still refer to `previous`.
    ScheduleChanged {
        previous: Rc<dyn Schedule>,
        current: Rc<dyn Schedule>,
        cause: ScheduleCause,
    },
    SourceReady(bool),
    SourceChanged,
    AccountStreams(Option<u32>),
    ForgetSource(SourceId),
    OpenPlaylists,
    ClosePlaylists,
    FirstRun,
    StoreStartupRecovered,
    StoreStartupFailed {
        path: Option<std::path::PathBuf>,
        error: super::Error,
    },
    Notify(&'static str),
    ResumeFullscreen,
    GuideChanged,
    LibraryChanged,
    SettingsChanged,
}
