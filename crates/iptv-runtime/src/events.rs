//! Application results applied by the shell through one event sink.
use iptv_core::guide::Schedule;
use iptv_library::SourceId;
use std::rc::Rc;

pub enum RuntimeEvent {
    RefreshDue(SourceId),
    CatalogChanged,
    EarlyResume {
        source: super::model::StoredSource,
        done: futures_channel::oneshot::Sender<()>,
    },
    Message(String),
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
        schedule: Rc<dyn Schedule>,
        snapshot: super::state::OrganizationSnapshot,
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
