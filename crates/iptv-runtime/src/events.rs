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
        has_vod: bool,
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
    Notify(&'static str),
    ResumeFullscreen,
    GuideChanged,
    LibraryChanged,
    SettingsChanged,
}
