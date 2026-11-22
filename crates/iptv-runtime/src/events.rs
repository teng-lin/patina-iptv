//! Application results applied by the shell through one event sink.
use iptv_core::guide::Schedule;
use iptv_library::SourceId;
use std::rc::Rc;

pub enum RuntimeEvent {
    ResumeChannel(usize),
    SchedulePresented(Vec<(&'static str, std::time::Duration)>),
    CatalogChanged,
    EarlyResume {
        source: Box<super::model::StoredSource>,
        done: futures_channel::oneshot::Sender<()>,
    },
    Message(String),
    SettingsSaveFailed(super::settings::SaveFailure),
    BackupProgress(super::backup::Progress),
    FavoritesChanged {
        channels: std::collections::BTreeSet<usize>,
        /// The source on screen has movies or shows (Movies, Shows).
        has_vod: bool,
        /// Some enabled source has movies or shows (the cross-source
        /// Continue watching and Recently added shelves).
        any_vod: bool,
    },
    FavoriteOrderChanged,
    RefreshProgress(SourceId, Option<super::library::Activity>),
    BrowsingAll(bool),
    ShownSource(Option<SourceId>),
    ScheduleReady(Rc<dyn Schedule>),
    OrganizationReady {
        source: SourceId,
        ticket: u64,
        schedule: Rc<dyn Schedule>,
        replacement: Rc<iptv_guide::EpgSchedule>,
    },
    SourceReady(bool),
    SourceChanged,
    AccountStreams(Option<u32>),
    ForgetSource(SourceId),
    OpenPlaylists,
    ClosePlaylists,
    FirstRun,
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
